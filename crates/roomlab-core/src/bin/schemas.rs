//! Generate versioned JSON Schema files in `contracts/` from the Rust contract types.
//! Usage: cargo run -p roomlab-core --bin roomlab-schemas -- <out_dir>

use roomlab_core::contracts::*;
use schemars::schema_for;

fn main() {
    let out = std::env::args().nth(1).unwrap_or_else(|| "contracts".into());
    std::fs::create_dir_all(&out).expect("create out dir");
    let schemas = vec![
        ("event_v1", serde_json::to_value(schema_for!(EventEnvelope)).unwrap()),
        ("message_v1", serde_json::to_value(schema_for!(MessageRecord)).unwrap()),
        ("agent_v1", serde_json::to_value(schema_for!(AgentProfile)).unwrap()),
        ("spawn_request_v1", serde_json::to_value(schema_for!(SpawnRequest)).unwrap()),
        ("action_request_v1", serde_json::to_value(schema_for!(ActionRequest)).unwrap()),
        ("action_result_v1", serde_json::to_value(schema_for!(ActionResult)).unwrap()),
        ("observation_v1", serde_json::to_value(schema_for!(Observation)).unwrap()),
        ("world_v1", serde_json::to_value(schema_for!(WorldSpec)).unwrap()),
        ("experiment_v1", serde_json::to_value(schema_for!(RunManifest)).unwrap()),
        ("provider_v1", serde_json::to_value(schema_for!(ProviderConfig)).unwrap()),
        ("intent_card_v1", serde_json::to_value(schema_for!(IntentCard)).unwrap()),
    ];
    for (name, mut v) in schemas {
        v["$id"] = serde_json::json!(format!("https://roomlab.local/contracts/{name}.schema.json"));
        v["x-schema-version"] = serde_json::json!(SCHEMA_VERSION);
        let path = format!("{out}/{name}.schema.json");
        std::fs::write(&path, serde_json::to_string_pretty(&v).unwrap()).expect("write schema");
        println!("wrote {path}");
    }
}
