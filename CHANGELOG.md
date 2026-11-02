# Changelog

## 0.1.0-beta.1 — 2026-11-02
First functional beta built from the ROOMLAB Master Build Blueprint v1.0 (gates G0–G8).
- Rust trusted core with SQLite event log, rooms, agents, observation-mediated contexts, scheduler,
  spawn governor, world authority, knowledge ACL + FTS5, S1 tool broker, Crucible, experiments.
- OpenAI-compatible streaming gateway; managed llama-server supervisor; reasoning allowance.
- Tauri 2 + React UI: first-run wizard, chat, Agent Foundry, Guild org/tasks, Story world, Crucible,
  Observatory, settings, diagnostics.
- Portable Windows packaging scripts; GUI tests against the real executable.
- Fixed during testing: scheduler aging cap could starve lowest-priority jobs (T16); demo quotes
  re-triggered mentions; thinking models returned empty replies without a reasoning allowance.
