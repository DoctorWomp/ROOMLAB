# Wire contracts (schema 1.0)

Source of truth: `crates/roomlab-core/src/contracts.rs` (serde types with `deny_unknown_fields` and
closed enums). JSON Schemas in `/contracts` are **generated** from those types:

```powershell
cargo run -p roomlab-core --bin roomlab-schemas -- contracts
```

| Schema | Rust type | Spec |
|---|---|---|
| `event_v1.schema.json` | `EventEnvelope` | §13.2 |
| `message_v1.schema.json` | `MessageRecord` | §13.3 |
| `observation_v1.schema.json` | `Observation` | §13.4 |
| `action_request_v1.schema.json` / `action_result_v1.schema.json` | `ActionRequest` / `ActionResult` | §13.5 |
| `spawn_request_v1.schema.json` | `SpawnRequest` | §7.3 |
| `agent_v1.schema.json` | `AgentProfile` | §5.3 |
| `world_v1.schema.json` | `WorldSpec` | §6.2 |
| `experiment_v1.schema.json` | `RunManifest` | §13.1, §15 |
| `provider_v1.schema.json` | `ProviderConfig` (api_key never serialized) | §14 |
| `intent_card_v1.schema.json` | `IntentCard` | §10.6 |

Round-trip and invalid-payload tests: `tests/g1_core_persistence.rs::contracts_round_trip_and_reject_invalid`
(unknown fields, unknown enum values, invalid world graphs, secret non-serialization).

Extensions beyond the spec examples (all optional fields): `MessageRecord.reaction_lane`,
`confidence`, `is_demo`, `model_id`, `seq`; `ActionRequest.reaction_lane`, `confidence`;
`WorldSpec.hidden_edges`, `sounds`, `start_location`, `inventory_capacity`; `WorldObject.notice_dc`,
`trace_on_take`, `found_with`, `known_by`; `ProviderConfig.reasoning_allowance_tokens`.

Reason codes (§13.5) plus `PENDING_APPROVAL` and `QUEUED` for spawn state reporting.
Migrations: forward-only list in `db.rs`; a `VACUUM INTO` backup is written before upgrading an existing DB.
