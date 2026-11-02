# RoomLab user guide

## The window

- **Top bar:** run menu (runs, scenario pack, import), current room, inference mode badge
  (`DEMO MODE` / `LOCAL · <model>` / `REMOTE · <model>`), **Run / Pause / Step**, **Cancel gen**
  (cancels the generation currently streaming; nothing is committed), **Fork**, **Observatory**,
  **Operator / Participant view**, Settings, Diagnostics, About.
- **Left:** rooms (＋ to create from a template), members of the current room with status
  (green active, amber dormant), `DEMO` (scripted) or `L1 · model` badges, sub-agents nested under
  their parent, and pending spawn approvals.
- **Centre:** chat. Every message carries an owner label: `HUMAN`, `AGENT`, `NARRATOR`, `WORLD`,
  `TOOL`, `SYSTEM`. Scripted output is tagged `DEMO`; model output is tagged with the model id.
  🔒 marks a message with a limited audience. Streaming model output appears as a dashed
  "unvalidated draft" and is only saved once the action gate accepts it.
- **Right:** inspector for the agent you click (profile, knowledge, history, spawns, exact prompt).
- **Status bar:** identities vs queued turns vs active generation vs model-loaded (only if the
  provider reports it, otherwise "unknown"), run budget, errors, event number.

## Composer

Enter sends, Shift+Enter adds a line. Type `@` to mention (↑/↓ + Tab). The visibility selector sends
to everyone or privately to one member. 🖼 attaches an image (PNG/JPEG/WebP/GIF ≤ 8 MB); agents
without a vision route only learn that an image is present.

## Agents

- **Demo (L0):** deterministic scripted policies. No model.
- **L1 reactive:** model is called only when the agent is mentioned, messaged directly or briefed.
- **L2 agentic:** model also gets group turns.
Create/edit in the **Agent Foundry** (＋ agent / Edit). Description detail and behavioural rigidity
are separate controls. Allowed actions are explicit grants — org rank grants nothing.
Suspend makes an agent dormant (no compute; it still receives messages and wakes with that history).

## Rooms

- **Commons:** group chat. Lumen answers the social bid first, Moth pokes holes with reaction lanes,
  Sable often stays silent on purpose (logged as an explicit NO_OP).
- **Guild:** post a task. The coordinator creates a task cluster and briefs specialists; the mediator
  synthesizes; the coordinator requests a **Counterexample Scout** sub-agent which needs your
  approval (left panel or Org & tasks). The scout gets only the handed-off brief, reports privately to
  its parent and is archived. Only you can mark a task VERIFIED.
- **Story:** World tab → map, your surroundings, Move/Look/Inspect/Take/Drop/Use. **Advance world
  tick** gives every character a turn. The world engine decides outcomes; the narrator only adds
  atmosphere. Characters perceive only their location (try Inspect → "the room" in the Cellar).
- **Crucible:** Cases tab → submit a proposal; steelman → skeptic → mediator take turns; export the
  record. Sealed intent: fill the intent card and signal; receivers answer blind; Reveal runs
  reconciliation.

## Operator vs participant view

Operator view shows private messages, operator-only notices, world truth and who-knows-what with an
`OPERATOR TRUTH` banner. Participant view shows only what the Human identity observed. The backend
enforces this. In a controlled trial, switching to operator view requires acknowledgement and marks
the run contaminated.

## Observatory

Events (filterable, expandable JSON), Who knows what, Manifest, Playback (slider; no model calls),
Metrics (measured p50/p95 from real calls only), Export/Import, Director (journaled interventions).

## Inference settings

Settings → Inference: Test /models lists the server's actual model ids. API keys are kept in memory
only. Remote endpoints require ticking an explicit acknowledgement. If the provider fails, RoomLab
reports `MODEL_UNAVAILABLE`, cancels queued model turns and never substitutes a model or invents a
reply. Advanced → Managed llama-server starts your own llama-server bound to 127.0.0.1.

## Scenario pack

Run menu → Scenario pack: C001 signal bid, C002 held open, C003 image (scripted; real vision blocked),
G001 roles, G002 spawn control (1 slot), G003 compete/cooperate (controlled), S001 library secret,
S002 summon relay, S003 hidden passage, X001 crucible, X002 sealed intent, SEC001 private-note leak test.
