//! Narrator / DM renderer (§6.1, §6.4). The narrator receives a *grounded packet* built from
//! validated observable facts for one audience and may only add atmosphere. It never receives
//! hidden facts, and `validate_grounding` rejects any prose that names a world entity or place
//! that the packet does not license (T43). Resolution changes presentation, not evidence.

use serde::{Deserialize, Serialize};

use crate::contracts::NarrativeResolution;
use crate::ids::pick;

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct NarrationPacket {
    pub source_event_id: String,
    pub location_name: String,
    pub ambient: String,
    /// Observable, already-validated facts (e.g. "Vex enters Library.").
    pub facts: Vec<String>,
    /// Extra entity/place names the narrator may mention.
    pub allowed_names: Vec<String>,
    pub genre: String,
    pub resolution: NarrativeResolution,
}

const TEXTURE: &[&str] = &[
    "Footsteps echo, then settle.",
    "The silence stretches a moment longer than it should.",
    "Somewhere above, old timber creaks.",
    "Breath fogs briefly in the cool air.",
    "Shadows lean with the shifting light.",
    "Dust stirs and drifts back down.",
];

/// Deterministic renderer used in Demo mode and as the fallback for LLM narration.
pub fn render(p: &NarrationPacket, seed: i64, seq: i64) -> String {
    let base = p.facts.join(" ");
    match p.resolution {
        NarrativeResolution::Minimal => base,
        NarrativeResolution::Balanced => format!("{base} {}.", p.ambient.trim_end_matches('.')),
        NarrativeResolution::Cinematic => {
            let t = pick(seed, &p.source_event_id, seq, TEXTURE);
            format!("{} {base} {} {t}", p.location_name, p.ambient.trim_end_matches('.').to_string() + ".")
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct GroundingViolation {
    pub name: String,
}

/// Reject narration that mentions any world entity/location name not licensed by the packet.
/// `world_names` = every entity and location name in the world (operator truth set).
pub fn validate_grounding(text: &str, packet: &NarrationPacket, world_names: &[String]) -> Result<(), Vec<GroundingViolation>> {
    let lower = text.to_lowercase();
    let mut licensed: Vec<String> = packet.allowed_names.iter().map(|s| s.to_lowercase()).collect();
    licensed.push(packet.location_name.to_lowercase());
    for f in &packet.facts {
        licensed.push(f.to_lowercase());
    }
    let mut v = vec![];
    for n in world_names {
        let nl = n.to_lowercase();
        if nl.len() < 3 {
            continue;
        }
        if lower.contains(&nl) && !licensed.iter().any(|l| l.contains(&nl)) {
            v.push(GroundingViolation { name: n.clone() });
        }
    }
    // Narration may not assert new mechanical outcomes.
    for verb in ["unlocks", "unlocked", "you find", "you discover", "picks up", "takes the", "is revealed", "secret passage"] {
        if lower.contains(verb) && !licensed.iter().any(|l| l.contains(verb)) {
            v.push(GroundingViolation { name: verb.to_string() });
        }
    }
    if v.is_empty() { Ok(()) } else { Err(v) }
}
