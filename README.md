# RoomLab — local-first multi-agent chat & simulation workbench (Windows beta)

RoomLab is a Discord-like chat surface over an auditable simulation kernel: several agents share
rooms, but each one only sees what was actually delivered to it. A trusted Rust core owns identities,
permissions, world truth, budgets and sub-agent spawning; model output is treated as untrusted data.

- **Four room types:** 💬 Commons (group chat), 🧩 Guild (coordinator + specialists, task clusters,
  approved sub-agents), 📜 Story (text world with rooms, objects, hidden facts and a world authority),
  🧪 Crucible (steelman / skeptic / mediator review, sealed intent tests).
- **Demo Mode** works with no model: deterministic scripted agents, always labelled `DEMO`.
- **Real local inference** through any OpenAI-compatible server (LM Studio, Ollama) with streaming,
  or a RoomLab-managed `llama-server` you point at your own GGUF file.
- **Observatory:** event log, who-knows-what (operator truth), run manifest, measured latency,
  export/import, deterministic playback, fork, Director interventions with contamination tracking.

Built from `docs/MASTER_BUILD_BLUEPRINT.md` (ROOMLAB Master Build Blueprint v1.0).
Status per feature and per acceptance test: **[docs/TEST_REPORT.md](docs/TEST_REPORT.md)** and
**[docs/KNOWN_LIMITATIONS.md](docs/KNOWN_LIMITATIONS.md)**.

## Quick start (end users)

1. Unzip `RoomLab_Win64_Portable.zip` anywhere you can write (e.g. Documents).
2. Run `RoomLab.exe` → **Explore Demo**. No installs needed on Windows 11 (WebView2 is preinstalled).
3. Optional: start LM Studio's server (Developer tab → Start Server), then Settings → Inference →
   Test /models → pick a model. For thinking models (Gemma 4, Qwen3) set **Reasoning allowance** to 800–1500.

See [docs/USER_GUIDE.md](docs/USER_GUIDE.md).

## Build from source (developers)

```powershell
powershell -ExecutionPolicy Bypass -File tools\build_windows.ps1
```

Prerequisites, exact commands and test commands: [docs/BUILD_AND_RUN.md](docs/BUILD_AND_RUN.md).

## Repository layout

| Path | What |
|---|---|
| `crates/roomlab-core/` | Trusted control plane (Rust): SQLite event log, rooms, agents, scheduler, spawn governor, world authority, knowledge ACL + FTS5, provider gateway, Crucible, experiments. 53 automated tests + 2 opt-in real-inference tests in `tests/`. |
| `src-tauri/` | Tauri 2 desktop shell: allowlisted command API, simulation runner, WebView2 check. |
| `src/` | React + TypeScript UI. |
| `contracts/` | Versioned JSON Schemas generated from the Rust contract types. |
| `templates/` | Room templates; `story/forgotten_keep/world.json`. |
| `tests/e2e/` | GUI tests that drive the real `RoomLab.exe` over WebView2 DevTools. |
| `tools/` | Windows build, packaging, smoke scripts; optional `export_run.py`. |
| `docs/` | Spec copy, decisions, contracts, security boundaries, guides, test report, screenshots. |

License: MIT for RoomLab source (see `LICENSES.md`). Third-party notices are generated into the release.
