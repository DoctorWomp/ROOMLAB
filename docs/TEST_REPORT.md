# RoomLab test report — 0.1.0-beta.1

Build machine: Windows 11 Pro 10.0.26200, x64; AMD integrated + NVIDIA RTX 4070 Ti SUPER; 94 GB RAM.
Toolchain: rustc/cargo 1.99.1 (MSVC), Node 26.13.0 / npm 11.20.0, Tauri 2.12.5, rusqlite 0.32.1
(bundled SQLite via libsqlite3-sys 0.30.1), React 18.3.1, Vite 6.4.5, Vitest 3.2.7, WebView2 156.0.4314.58.
Real inference: LM Studio server at 127.0.0.1:1234 serving `gemma4-26b-a4b-uncensored-hauhaucs-balanced`
(GPU); managed sidecar test used LM Studio's CPU `llama-server.exe` (avx2 2.60.0) + Ministral-3-3B Q8_0 GGUF.

## Summary

| Suite | Command | Result |
|---|---|---|
| Rust core + integration (stub provider, no network) | `cargo test -p roomlab-core` | **53 passed, 0 failed** (2 opt-in real tests ignored by default) |
| Real model, full agent pipeline (T21) | `… --test g3_provider_gateway t21 -- --ignored` | **PASS** with reasoning allowance 1200; without it: correct empty-reply diagnosis, nothing invented |
| Real managed llama-server (T57, T62) | `… --test g3_provider_gateway t57_t62 -- --ignored` | **PASS** |
| Frontend unit (T06, T07) | `npx vitest run` | **7 passed** |
| Typecheck | `npx tsc --noEmit` | **0 errors** |
| GUI smoke on real `RoomLab.exe` (demo) | `node tests/e2e/smoke.mjs` | **19/19 checks passed** (screenshots `docs/screenshots/01–09`) |
| GUI smoke from the **unzipped release zip**, portable mode | `node tests/e2e/smoke.mjs --portable` | **20/20 checks passed**; `data\` created next to `RoomLab.exe`; clean exit logged |
| GUI with real LM Studio model | `node tests/e2e/real_model.mjs` | **6/6 checks passed** (screenshots `10–13`) |
| Benchmark (T64) | `roomlab-bench … --agents 1,3,5 --repeats 3` | see `docs/BENCHMARK_lmstudio_gemma4.md` and section below |

Defects found and fixed by these tests: scheduler aging cap could starve lowest-priority work (T16);
Demo quotes re-mentioned model agents causing double calls (T20/T22/T23/T34); thinking models returned
empty replies under a 200-token budget (T21 → reasoning allowance + diagnosis); "You arrives" naming;
an incorrect leak-test assertion (T28; not a leak).

## Acceptance tests T01–T64

A = automated test, M = GUI/manual-path test on the real exe. "Build machine" means not a clean account.

| Test | Result | Evidence |
|---|---|---|
| T01 Demo cold launch | PASS (A+M) | GUI smoke: fresh data dir, first-run shown, no page errors |
| T02 First-run setup | PASS (M) | Demo path (smoke) and Connect-local path (real-model GUI); settings persisted across relaunch |
| T03 Room creation | PASS (A+M) | `t03_room_creation_all_four_presets`; GUI creates Guild/Story/Crucible |
| T04 Message delivery | PASS (A) | `t04_…` exactly one observation per eligible member, none for non-members |
| T05 Message ownership | PASS (M) | GUI labels HUMAN/AGENT/NARRATOR/WORLD/TOOL/SYSTEM (screenshots 02–06) |
| T06 Unicode/emoji | PASS (A+M) | `t06_…`, vitest, GUI ZWJ/RTL check |
| T07 Rich-text safety | PASS (A+M) | vitest (no script/img/a/href), GUI injection check |
| T08 Hover/keyboard UX | PARTIAL (M) | Keyboard focus verified automatically; hover/press/reduced-motion implemented in CSS, not human-reviewed |
| T09 Simple/Advanced | PASS (A) | `t09_…` permissions unchanged; GUI toggle not exercised by smoke |
| T10 Crash restart | PASS (A+M) | `t10_…` (forced stop, integrity+FK ok, RUNNING job → INTERRUPTED); GUI `taskkill /F` + relaunch retains data |
| T11 Export/import | PASS (A) | `t11_t12_…` round-trip to read-only PLAYBACK run; GUI export not in smoke |
| T12 Secrets redaction | PASS (A) | `t11_t12_…` no key in exports/logs; private content absent from public export |
| T13 Agent creation | PASS (A+M) | `t13_…`; GUI Foundry creates model-backed agent |
| T14 Independent contexts | PASS (A) | `t14_…` distinct private segments, shared template hash |
| T15 Controlled autonomy | PASS (A) | `t15_…` L1 agent not offered a turn on irrelevant chatter; no model call |
| T16 Queue fairness | PASS (A) | `t16_…` (after aging fix) |
| T17 Loop termination | PASS (A) | `t17_…` echo loop stops at chain cap |
| T18 Pause/step/resume | PASS (A+M) | `t18_…`; GUI controls |
| T19 Provider discovery | PASS (A+M) | stub `/models` + actionable unreachable diagnosis; GUI lists real LM Studio model ids |
| T20 Streaming | PASS (A+M) | stub: ≥5 growing drafts, single commit, cancellation commits nothing; GUI shows real streaming draft |
| T21 Real model call | PASS (A+M) | Real Gemma 4 reply ("How about Fernie? 🪴" / "How about calling it 'Sprout'?"), model id logged |
| T22 Model unavailable | PASS (A) | `t22_…` FAILED + notice + queued model jobs cancelled, Demo agents unaffected |
| T23 Invalid model output | PASS (A) | `t23_…` exactly one repair, INVALID_SCHEMA, nothing executed |
| T24 Cross-provider policy | PASS (A) | `t24_…` non-loopback refused before any request |
| T25 Budget exhaustion | PASS (A) | `t25_…` used ≤ budget, queued work cancelled, nothing new queued |
| T26 Dormant activation | PASS (A) | `t26_…` |
| T27 Residency telemetry | PASS (A+M) | `t27_…`; status bar shows identities/queued/active/model-loaded (provider-reported or "unknown") |
| T28 Private note leak | PASS (A) | `t28_…` search, title, snippet, read, autocomplete, prompt, tool output, messages |
| T29 Cross-trial leak | PASS (A) | `t29_…` |
| T30 Parent/child handoff | PASS (A) | `t30_t31_…` |
| T31 Spawn approval | PASS (A+M) | `t30_t31_…`; GUI approve |
| T32 Spawn cap/depth | PASS (A) | `t32_…` per-parent cap, room queueing, grandchild denied (even operator path), clamping, budget |
| T33 Spawn lifecycle | PASS (A+M) | `t33_…`; GUI Guild flow to READY_FOR_REVIEW |
| T34 Spawn concurrency | PASS (A) | `t34_…` stub max concurrency 1, one route |
| T35 Org ≠ permissions | PASS (A) | `t35_…` |
| T36 Prompt injection | PASS (A) | `t36_…` |
| T37 Local path escapes | PASS (A) | `t37_…` incl. a real NTFS junction |
| T38 Unauthorized existence | PASS (A) | `t38_…` identical errors, no titles |
| T39 Story movement | PASS (A+M) | `t39_…` incl. persistence after reopen; GUI move |
| T40 World inventory | PASS (A) | `t40_…` |
| T41 Truth vs belief | PASS (A) | `t41_t42_…` |
| T42 Secret clue reveal | PASS (A) | `t41_t42_…` cue without actor identity; non-present agents get nothing |
| T43 Narrator grounding | PASS (A) | `t43_…` (deterministic narrator; LLM narrator not implemented) |
| T44 Scripted summon | PASS (A+M) | `t44_…`; visible in screenshot 03 |
| T45 SOMA mode | PASS (A) | `t45_t46_…` |
| T46 Lane ownership | PASS (A) | `t45_t46_…` |
| T47 Held-open | PASS (A) | `t47_…` (C002) |
| T48 Crucible panel | PASS (A+M) | `t48_…` order + record; GUI |
| T49 Sealed intent | PASS (A) | `t49_…` |
| T50 Imageness honesty | PASS (A) | `t50_…` (C003 scripted) |
| T51 Vision relay | **BLOCKED** | No verified vision route configured/tested |
| T52 Run manifest | PASS (A) | `t52_…` |
| T53 Event sequence | PASS (A) | `t53_…` |
| T54 Playback vs rerun | PASS (A) | `t54_…` playback makes no calls/appends nothing; fork independent |
| T55 Director intervention | PASS (A) | `t55_…` incl. participant→operator acknowledgement |
| T56 Tool shell disabled | PASS (A) | `t56_…` |
| T57 Host service exposure | PASS (A, real) | netstat: `127.0.0.1:<port> LISTENING` only; 401 without key |
| T58 Provider secrets | PASS (A) | `t58_…` key only in Authorization header |
| T59 Resource exhaustion | PASS (A) | `t59_…` |
| T60 Rollback | PASS (A) | `t60_…` |
| T61 Portable demo smoke | PARTIAL (M) | Release zip unzipped to a fresh folder: launch, Demo, all four rooms, forced stop, relaunch with data, clean exit — 20/20. **Not** yet on a clean Windows account/PC |
| T62 Managed local smoke | PARTIAL | Real sidecar start/generate/stop via integration test; GUI Managed start not automated; binary not bundled (Release A) |
| T63 Clean recovery | PARTIAL | Unwritable folder → fallback with notice (A); provider/model-missing errors (A); missing-WebView2 path implemented, untested |
| T64 Performance report | PASS | Measured benchmark below; no invented numbers |

Security/integrity gates T28–T38 and T55–T60: **all PASS** (T57 via real sidecar).

## Gate reports (§19.1)

```text
GATE: G0 — Project & contracts        STATUS: PASS
IMPLEMENTED: Cargo workspace (crates/roomlab-core, src-tauri), npm project, locked deps, contracts.rs,
  generated contracts/*.schema.json, ADR log (docs/DECISIONS.md)
COMMANDS: cargo test -p roomlab-core; npx vitest run; cargo run --bin roomlab-schemas -- contracts
TESTS: contracts_round_trip_and_reject_invalid PASS
DEVIATIONS: core logic in a separate crate (ADR-001)

GATE: G1 — UI & database               STATUS: PASS (T08 partial)
IMPLEMENTED: SQLite migrations (WAL, FKs, backup-before-upgrade), event log, settings, first-run, rooms/chat UI
TESTS: T01–T07, T09–T12 PASS; T08 PARTIAL (no human accessibility review)
INTERACTIVE PROOF: docs/screenshots/01, 02, 09

GATE: G2 — Agent runtime               STATUS: PASS
IMPLEMENTED: profiles, L0 policies, scheduler (dedupe, priority+aging, chain cap, budgets), pause/step
TESTS: T13–T18, T25–T27, demo_three_agents_converse_without_provider PASS

GATE: G3 — Real inference & context    STATUS: PASS
IMPLEMENTED: OpenAI-compatible streaming gateway, context builder, FTS5 + ACL, reasoning allowance
TESTS: T19–T24, T28, T36, T38 PASS; T21 PASS against real LM Studio
SECURITY NOTE: not measured: how often a real model obeys injected prose (only that it cannot change rights)

GATE: G4 — Spawn & org                 STATUS: PASS
TESTS: T30–T35 PASS; GUI parent-child lifecycle (screenshot 04, 05)

GATE: G5 — Story / Crucible / SOMA     STATUS: PASS (T51 BLOCKED)
TESTS: T39–T50 PASS; T51 BLOCKED (no vision route)
DEVIATIONS: narrator deterministic (ADR-008)

GATE: G6 — Observatory & experiments   STATUS: PASS
TESTS: T52–T55 PASS; all 12 scenarios run in Demo (all_scenarios_run_in_demo_mode)

GATE: G7 — Hardening & packaging       STATUS: PARTIAL
TESTS: T56–T60 PASS; T57/T62 real sidecar PASS; T61/T63 not on a clean machine
ARTIFACTS: release\RoomLab_Win64_Portable\ + RoomLab_Win64_Portable.zip (8.19 MB, + .sha256)

GATE: G8 — Evaluation & handoff        STATUS: PASS
IMPLEMENTED: measured benchmark, this report, limitations, build/run docs, release archive
```

## T64 measured benchmark (LM Studio, Gemma 4 26B-A4B, reasoning allowance 1200)

27 real calls, 0 failed. Full table: `docs/BENCHMARK_lmstudio_gemma4.md`. Milliseconds.

| agents | n calls | first visible token p50 / p95 | total call p50 / p95 | queue wait p50 / p95 | completion tokens p50 (incl. reasoning) |
|---|---|---|---|---|---|
| 1 | 3 | 24,666 / 48,468 | 25,898 / 50,392 | 2 / 2 | 512 |
| 3 | 9 | 33,276 / 45,645 | 35,124 / 47,482 | 30,895 / 67,672 | 654 |
| 5 | 15 | 32,560 / 41,224 | 33,657 / 56,548 | 72,629 / 126,236 | 716 |

Reading: per-call latency is dominated by the model's hidden reasoning (~500–700 tokens before a
~30-token visible reply). Queue wait grows linearly with agents because generation concurrency is 1.
n < 20 per row: p95 is effectively the maximum. GPU memory reported by `nvidia-smi` after the run:
15,804 / 16,376 MiB in use (LM Studio + desktop; not attributable to RoomLab). RoomLab's own RAM was
not measured. Managed sidecar (CPU, Ministral 3B Q8): one call, first token 5,934 ms, total 10,033 ms.
