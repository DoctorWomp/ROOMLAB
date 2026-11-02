# Architecture decisions (ADR log)

**ADR-001 Workspace split.** The trusted core is a separate crate (`crates/roomlab-core`) and the
Tauri crate (`src-tauri`) is a thin, allowlisted command layer. Rationale: the security-relevant
logic is testable with `cargo test` without a window; the spec's `src-tauri/src/*.rs` module list
maps to `crates/roomlab-core/src/*.rs` (`comms.rs`, `scheduler.rs`, `spawn.rs`, `knowledge.rs`,
`world.rs`, `narrative.rs`, `soma.rs`, `experiments.rs`, `providers/`, `tools.rs` = `tools/broker.rs`).
`state.rs`/`auth.rs`/`migrations.rs` are `lib.rs`, `auth.rs`, `db.rs`.

**ADR-002 Run-scoped composite keys.** Every entity row is keyed by `(run_id, id)`. Fork = copy rows
with a new run id; cross-trial isolation (T29) falls out of every query being run-scoped.

**ADR-003 One authoritative log.** `events.seq` is allocated with `UPDATE … RETURNING` inside the
mutation's transaction, so a rolled-back action never consumes a seq (T53/T60). JSONL is export-only.

**ADR-004 Observation-mediated context.** Agents never read `messages` directly. Delivery writes an
observation per eligible recipient; the context builder joins only the reader's observations. This
makes "what was delivered" auditable and is the basis of the leak tests.

**ADR-005 Authorization inside SQL.** Knowledge search applies the ACL predicate in the FTS query's
WHERE clause, so titles/snippets of unauthorized rows are never produced (T28/T38). Unauthorized and
nonexistent reads return the same error.

**ADR-006 Plain prose = SPEAK.** Model replies with no `{` are accepted as SPEAK (lowest privilege)
when the room allows it (default on; off in Story). Any JSON-shaped reply must validate strictly
(`deny_unknown_fields`, closed enums); otherwise one bounded repair request, then NO_OP + INVALID_SCHEMA.

**ADR-007 Reasoning allowance.** Real testing with Gemma 4 26B showed thinking models can spend the
whole output budget on hidden reasoning (197/200 tokens) and return no content. RoomLab adds a
configurable allowance on top of the visible ceiling, charges actual usage, records reasoning tokens,
and posts an operator notice when a reply is empty for this reason. Nothing is invented.

**ADR-008 Deterministic narrator.** The M beta narrates with a deterministic renderer over a grounded
packet; an LLM narrator is FUTURE. The grounding validator (T43) is implemented and tested.

**ADR-009 Secrets in memory only.** Provider keys are never serialized (`#[serde(skip)]`), never
written to settings/DB/exports, and are redacted from logs/exports. Users re-enter keys per session.
(OS credential storage is a later enhancement.)

**ADR-010 Concurrency 1.** One runner task processes one job at a time (spec default). Identities
are multiplexed on one route without per-identity model processes (T34).

**ADR-011 Scheduler fairness.** Effective priority = `priority×20 − min(age_in_seq, 240) + min(usage,10)`.
The first version capped aging at 60 — equal to the largest priority gap — and T16 caught that a
lowest-priority job could starve. Cap raised to 240.

**ADR-012 Portable exe, no installer.** `tauri build --no-bundle` + `tools/package_portable.ps1`.
The UI is embedded in `RoomLab.exe`; WebView2 is an OS prerequisite with a native error box if missing.

**ADR-013 GUI testing over CDP.** GUI tests drive the real `RoomLab.exe` through WebView2's
DevTools port with `playwright-core` (no bundled browser). This tests the shipped binary, not a mock.

**ADR-014 Human identity.** The operator participates as `human_01` ("Human"), a member of every room
and a character in Story worlds, subject to the same world rules.
