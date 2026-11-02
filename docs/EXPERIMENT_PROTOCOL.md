# Experiment protocol

1. **Choose a scenario** (run menu → Scenario pack) or build rooms manually in a sandbox run.
   Tick "controlled trial" for anything you intend to compare.
2. **Freeze conditions.** The run manifest records seed, model routes (provider, model id, endpoint
   host), budgets, spawn policy, app/schema versions and condition arms at creation.
3. **Run.** Use Pause/Step for turn-by-turn observation. Every turn records whether it was offered,
   suppressed by policy, queued, generated or an explicit NO_OP — silence is never confused with
   "not offered".
4. **Interventions.** Director edits, settings changes, model-route changes, room-setting changes and
   opening operator truth during a controlled trial are journaled with before/after state and mark the
   run `contaminated` unless pre-authorized.
5. **Sealed intent (X002).** The card is stored as an evaluator-only answer key before receivers see
   the signal; reveal triggers reconciliation turns. Lane visible vs withheld is a recorded arm.
6. **Analysis.** Observatory → Export (public or operator). `tools/export_run.py` prints per-agent
   turn outcomes and measured latencies from an export. Playback reads recorded events (no model
   calls); Rerun = start the same scenario again (outputs may differ); Fork = copy current state into
   a new run id.

Caveats: the same seed does not make model replies identical. Scripted (DEMO) agents are not models.
Confidence labels and reaction lanes are communication conventions, not measurements of internal
states; human raters and held-out cases are needed for interpretive claims.
