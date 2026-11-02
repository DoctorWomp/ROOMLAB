# RoomLab 0.1.0-beta.1 — release notes

Release A, "Connected Portable" (Windows 11 x64). `RoomLab.exe` with embedded UI; Demo Mode needs
nothing else. Connect LM Studio/Ollama for real inference, or point Managed mode at your own
llama-server + GGUF. No model weights or inference binaries are included.

| Area | State |
|---|---|
| Demo Mode, four room types, chat, agents, scheduler, budgets, pause/step | TESTED |
| OpenAI-compatible streaming provider (LM Studio verified with Gemma 4 26B) | TESTED |
| Reasoning allowance for thinking models | TESTED |
| Knowledge ACL + FTS5, leak tests, prompt-injection boundary | TESTED |
| Spawn Governor (one-level task worker, approval, caps, TTL) | TESTED |
| Story world authority, hidden passage, clue observation, scripted familiar | TESTED |
| SOMA modes, reaction-lane ownership, HELD_OPEN | TESTED |
| Crucible roles, sealed intent envelopes | TESTED |
| Observatory, manifests, export/import, playback, fork, interventions | TESTED |
| Managed llama-server supervisor | PARTIAL (tested with user-supplied binary; not bundled) |
| Cloud endpoints | PARTIAL (opt-in path tested with policy tests only) |
| Vision relay | BLOCKED |
| S2/S3 sandboxes, recursive spawning, image generation | FUTURE |

Full evidence: `docs/TEST_REPORT.md`. Limitations: `docs/KNOWN_LIMITATIONS.md`.
