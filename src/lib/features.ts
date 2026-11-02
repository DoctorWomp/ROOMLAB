// Truthful feature-state labels shown in About → Limitations (§0.2). Keep in sync with docs/TEST_REPORT.md.
export type FeatureState = "IMPLEMENTED" | "TESTED" | "PARTIAL" | "BLOCKED" | "FUTURE";

export const FEATURES: { area: string; feature: string; state: FeatureState; note: string }[] = [
  { area: "Demo", feature: "Demo Mode (no model needed), scripted agents labelled DEMO", state: "TESTED", note: "Automated tests + GUI smoke" },
  { area: "Rooms", feature: "Commons, Guild, Story, Crucible presets", state: "TESTED", note: "T03" },
  { area: "Agents", feature: "Create / edit / delete / dormant / wake, stable IDs", state: "TESTED", note: "T13, T26" },
  { area: "Inference", feature: "OpenAI-compatible provider (LM Studio / Ollama) with streaming", state: "TESTED", note: "Stub-backed T19–T24; real LM Studio call verified (T21)" },
  { area: "Inference", feature: "Reasoning allowance for thinking models", state: "TESTED", note: "Real Gemma 4 run: empty reply without it is reported, never invented" },
  { area: "Inference", feature: "Managed llama-server sidecar", state: "PARTIAL", note: "Supervisor implemented; no llama-server binary is bundled — select your own" },
  { area: "Inference", feature: "Cloud endpoints", state: "PARTIAL", note: "Opt-in with explicit acknowledgement; keys held in memory only" },
  { area: "Knowledge", feature: "Agent-private notes, room docs, ACL-filtered FTS5 search", state: "TESTED", note: "T28, T36–T38" },
  { area: "Scheduler", feature: "Event-driven turns, pause/step/resume, budgets, loop caps", state: "TESTED", note: "T15–T18, T25" },
  { area: "Spawn", feature: "One-level task worker with approval, caps, TTL, archive", state: "TESTED", note: "T30–T35" },
  { area: "Spawn", feature: "Agent-spawned room participants, recursive spawning", state: "FUTURE", note: "Blocked by design in this beta" },
  { area: "Story", feature: "World authority, movement, inventory, hidden passage, clues", state: "TESTED", note: "T39–T42" },
  { area: "Story", feature: "Scripted familiar summon (L0)", state: "TESTED", note: "T44" },
  { area: "Story", feature: "LLM narrator", state: "FUTURE", note: "Narration is deterministic; grounding validator is tested (T43)" },
  { area: "SOMA", feature: "Expression modes, reaction lanes with owners, COMMIT/LEAN/HELD_OPEN", state: "TESTED", note: "T45–T47. Not emotion detection." },
  { area: "Crucible", feature: "Steelman / skeptic / mediator rounds, case export", state: "TESTED", note: "T48" },
  { area: "Crucible", feature: "Sealed intent envelopes and reveal", state: "TESTED", note: "T29, T49" },
  { area: "Images", feature: "Attach/show images; no-vision honesty", state: "TESTED", note: "T50" },
  { area: "Images", feature: "Vision relay via a vision model", state: "BLOCKED", note: "No verified vision route configured" },
  { area: "Observatory", feature: "Event log, who-knows-what, manifest, export/import, playback, fork, interventions", state: "TESTED", note: "T52–T55" },
  { area: "Security", feature: "No shell / code / network tools (S1 tier only)", state: "TESTED", note: "T56" },
  { area: "Security", feature: "S2 OS sandbox / S3 VM workers", state: "FUTURE", note: "Not shipped" },
  { area: "Packaging", feature: "Portable Windows folder with RoomLab.exe", state: "PARTIAL", note: "Built and smoke-tested on the build machine; not yet tested on a clean Windows account" },
];
