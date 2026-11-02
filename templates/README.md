# RoomLab templates

Room templates are applied by the trusted core (`crates/roomlab-core/src/scenarios.rs`).
- `story/forgotten_keep/world.json` — the Story world (validated against `contracts/world_v1.schema.json`); it is compiled into RoomLab.exe so the shipped copy is for reference/editing.
- `commons/`, `guild/`, `crucible/` — describe the built-in agent line-ups. Their agents are defined in code so they stay schema-valid.
