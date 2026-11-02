//! Crucible (§4, §10.6, §15): steelman → skeptic → mediator rounds with an evaluator record
//! outside the participants, and sealed intent envelopes (X002) that receivers cannot read
//! until the operator reveals them (T49).

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::comms::{self, PostSpec};
use crate::contracts::*;
use crate::error::{CoreError, Result};
use crate::events::{self, NewEvent};
use crate::ids::new_id;
use crate::knowledge::{self, NewResource};
use crate::room;
use crate::scheduler::{self, EnqueueSpec, Job};

pub const ROLE_ORDER: &[&str] = &["steelman", "skeptic", "mediator"];

fn role_agent(conn: &Connection, run_id: &str, room_id: &str, role: &str) -> Result<Option<String>> {
    Ok(room::members(conn, run_id, room_id)?.into_iter().find(|m| m.role == role && m.status == "ACTIVE").map(|m| m.agent_id))
}

/// Start a case: post it as the human, then queue the first role. Later roles are queued
/// by `advance` when the previous role's job finishes, so order is guaranteed.
pub fn start_case(conn: &Connection, run_id: &str, room_id: &str, case_text: &str, rounds: i64) -> Result<String> {
    if room::room_kind(conn, run_id, room_id)? != RoomKind::Crucible {
        return Err(CoreError::Invalid("cases run in Crucible rooms".into()));
    }
    let rounds = rounds.clamp(1, 3);
    let case_id = new_id("case");
    let p = comms::post(conn, run_id, PostSpec {
        room_id: room_id.into(),
        author_id: crate::HUMAN_ID.into(),
        text: format!("📂 Case: {}", case_text.trim()),
        visibility: Some(AudienceScope::RoomMembers),
        chain_depth: 99, // orchestrated below, not by free reaction
        ..Default::default()
    })?;
    events::append(
        conn,
        run_id,
        NewEvent::new(EventType::CrucibleRound, OriginKind::Human, crate::HUMAN_ID, AudienceScope::RoomMembers, json!({
            "case_id": case_id, "round": 0, "case_text": case_text, "rounds": rounds, "case_message_id": p.message.message_id
        }))
        .room(room_id),
    )?;
    queue_role(conn, run_id, room_id, &case_id, 1, 0, &p.message.message_id, &p.event_id)?;
    Ok(case_id)
}

#[allow(clippy::too_many_arguments)]
fn queue_role(conn: &Connection, run_id: &str, room_id: &str, case_id: &str, round: i64, idx: usize, msg_id: &str, ev: &str) -> Result<()> {
    let role = ROLE_ORDER[idx];
    if let Some(agent) = role_agent(conn, run_id, room_id, role)? {
        let reason = format!("crucible:{case_id}:R{round}:{}", role.to_uppercase());
        scheduler::enqueue(conn, run_id, EnqueueSpec {
            agent_id: &agent,
            room_id,
            trigger_event_id: ev,
            trigger_message_id: Some(msg_id),
            priority: scheduler::PRIO_TASK,
            chain_depth: 0,
            reason: &reason,
        })?;
    }
    Ok(())
}

/// Called after a job finishes: record the round and queue the next role.
pub fn advance(conn: &Connection, run_id: &str, job: &Job, produced_message: Option<&str>) -> Result<()> {
    let Some(rest) = job.reason.strip_prefix("crucible:") else { return Ok(()) };
    let parts: Vec<&str> = rest.split(':').collect();
    if parts.len() != 3 {
        return Ok(());
    }
    let (case_id, round, role) = (parts[0], parts[1].trim_start_matches('R').parse::<i64>().unwrap_or(1), parts[2].to_lowercase());
    events::append(
        conn,
        run_id,
        NewEvent::new(EventType::CrucibleRound, OriginKind::System, "crucible", AudienceScope::RoomMembers, json!({
            "case_id": case_id, "round": round, "role": role, "agent_id": job.agent_id, "message_id": produced_message
        }))
        .room(&job.room_id),
    )?;
    let rounds: i64 = conn
        .query_row(
            "SELECT json_extract(payload_json,'$.rounds') FROM events WHERE run_id=?1 AND event_type='CRUCIBLE_ROUND' AND json_extract(payload_json,'$.case_id')=?2 AND json_extract(payload_json,'$.round')=0",
            params![run_id, case_id],
            |r| r.get(0),
        )
        .optional()?
        .unwrap_or(1);
    let idx = ROLE_ORDER.iter().position(|r| *r == role).unwrap_or(2);
    let (next_round, next_idx) = if idx + 1 < ROLE_ORDER.len() { (round, idx + 1) } else { (round + 1, 0) };
    if next_round > rounds {
        return Ok(());
    }
    let trig_msg = job.trigger_message_id.clone().unwrap_or_default();
    queue_role(conn, run_id, &job.room_id, case_id, next_round, next_idx, &trig_msg, &job.trigger_event_id)
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct CaseRecord {
    pub case_id: String,
    pub case_text: String,
    pub rounds: Vec<Value>,
    pub evaluator_note: String,
}

pub fn case_record(conn: &Connection, run_id: &str, case_id: &str) -> Result<CaseRecord> {
    let mut st = conn.prepare(
        "SELECT payload_json FROM events WHERE run_id=?1 AND event_type='CRUCIBLE_ROUND' AND json_extract(payload_json,'$.case_id')=?2 ORDER BY seq",
    )?;
    let rows = st.query_map(params![run_id, case_id], |r| r.get::<_, String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
    if rows.is_empty() {
        return Err(CoreError::NotFound(format!("case {case_id}")));
    }
    let mut case_text = String::new();
    let mut rounds = vec![];
    for r in rows {
        let v: Value = serde_json::from_str(&r)?;
        if v["round"] == 0 {
            case_text = v["case_text"].as_str().unwrap_or("").to_string();
            continue;
        }
        let mut entry = v.clone();
        if let Some(mid) = v["message_id"].as_str() {
            if let Ok(m) = comms::get_message(conn, run_id, mid) {
                entry["text"] = json!(m.record.text);
                entry["author"] = json!(m.author_name);
                entry["confidence"] = json!(m.record.confidence);
                entry["is_demo"] = json!(m.record.is_demo);
                entry["model_id"] = json!(m.record.model_id);
            }
        }
        rounds.push(entry);
    }
    let held = rounds.iter().filter(|r| r["confidence"] == "HELD_OPEN").count();
    Ok(CaseRecord {
        case_id: case_id.into(),
        case_text,
        evaluator_note: format!("{} role messages recorded; {} marked HELD_OPEN. Roles are labels for review, not validated reasoning traces.", rounds.len(), held),
        rounds,
    })
}

pub fn list_cases(conn: &Connection, run_id: &str, room_id: &str) -> Result<Vec<Value>> {
    let mut st = conn.prepare(
        "SELECT payload_json, seq FROM events WHERE run_id=?1 AND room_id=?2 AND event_type='CRUCIBLE_ROUND' AND json_extract(payload_json,'$.round')=0 ORDER BY seq DESC",
    )?;
    let v = st
        .query_map(params![run_id, room_id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?
        .into_iter()
        .filter_map(|(p, s)| serde_json::from_str::<Value>(&p).ok().map(|mut v| {
            v["seq"] = json!(s);
            v
        }))
        .collect();
    Ok(v)
}

// ───────────────────────────── Sealed intent envelopes (X002) ─────────────────────────────

pub fn seal_intent(conn: &Connection, run_id: &str, room_id: &str, author_id: &str, card: &IntentCard, message_text: &str, lane_visible: bool, lane: Option<ReactionLane>) -> Result<String> {
    for f in [&card.intended_stance, &card.intended_lane, &card.load_bearing_cue, &card.predicted_misread, &card.what_counts_as_hit] {
        if f.chars().count() > 500 {
            return Err(CoreError::Invalid("intent card fields are limited to 500 characters".into()));
        }
    }
    let env_id = new_id("intent");
    // 1) Sealed answer key goes to evaluator-only storage *before* receivers see anything.
    knowledge::add_resource(conn, run_id, NewResource {
        namespace: &format!("evaluator/{run_id}"),
        owner_id: "evaluator",
        title: &format!("Sealed intent card {env_id}"),
        body: &serde_json::to_string(card)?,
        kind: KnowledgeKind::HiddenTestKey,
        visibility: ResourceVisibility::Sealed,
        source_id: &env_id,
    })?;
    let seq = events::current_seq(conn, run_id)?;
    conn.execute(
        "INSERT INTO intent_envelopes(run_id, id, room_id, author_id, card_json, sealed, created_seq) VALUES (?1,?2,?3,?4,?5,1,?6)",
        params![run_id, env_id, room_id, author_id, serde_json::to_string(card)?, seq],
    )?;
    events::append(
        conn,
        run_id,
        // Payload deliberately omits the card.
        NewEvent::new(EventType::IntentSealed, OriginKind::Human, author_id, AudienceScope::OperatorOnly, json!({"envelope_id": env_id, "lane_visible": lane_visible})).room(room_id),
    )?;
    // 2) The signal itself; lane visible or withheld per treatment arm.
    let p = comms::post(conn, run_id, PostSpec {
        room_id: room_id.into(),
        author_id: author_id.into(),
        text: message_text.into(),
        visibility: Some(AudienceScope::RoomMembers),
        reaction_lane: if lane_visible { lane } else { None },
        chain_depth: 99,
        ..Default::default()
    })?;
    conn.execute("UPDATE intent_envelopes SET message_id=?3 WHERE run_id=?1 AND id=?2", params![run_id, env_id, p.message.message_id])?;
    for r in ROLE_ORDER {
        if let Some(agent) = role_agent(conn, run_id, room_id, r)? {
            scheduler::enqueue(conn, run_id, EnqueueSpec {
                agent_id: &agent,
                room_id,
                trigger_event_id: &p.event_id,
                trigger_message_id: Some(&p.message.message_id),
                priority: scheduler::PRIO_TASK,
                chain_depth: 0,
                reason: &format!("intent:{env_id}:RECEIVER"),
            })?;
        }
    }
    Ok(env_id)
}

pub fn reveal_intent(conn: &Connection, run_id: &str, env_id: &str) -> Result<()> {
    let (room_id, card, sealed, msg): (String, String, i64, Option<String>) = conn
        .query_row("SELECT room_id, card_json, sealed, message_id FROM intent_envelopes WHERE run_id=?1 AND id=?2", params![run_id, env_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
        .optional()?
        .ok_or_else(|| CoreError::NotFound(format!("envelope {env_id}")))?;
    if sealed == 0 {
        return Err(CoreError::Invalid("already revealed".into()));
    }
    let ev = events::append(conn, run_id, NewEvent::new(EventType::IntentRevealed, OriginKind::Human, crate::HUMAN_ID, AudienceScope::RoomMembers, json!({"envelope_id": env_id})).room(&room_id))?;
    conn.execute("UPDATE intent_envelopes SET sealed=0, revealed_seq=?3 WHERE run_id=?1 AND id=?2", params![run_id, env_id, ev.seq])?;
    let c: IntentCard = serde_json::from_str(&card)?;
    let p = comms::post(conn, run_id, PostSpec {
        room_id: room_id.clone(),
        author_id: "evaluator".into(),
        text: format!(
            "REVEAL ({env_id}) — intended stance: {}; intended lane: {}; load-bearing cue: {}; predicted misread: {}; counts as hit: {}",
            c.intended_stance, c.intended_lane, c.load_bearing_cue, c.predicted_misread, c.what_counts_as_hit
        ),
        visibility: Some(AudienceScope::RoomMembers),
        system_origin: Some(OriginKind::System),
        chain_depth: 99,
        parents: vec![ev.event_id.clone()],
        ..Default::default()
    })?;
    for r in ROLE_ORDER {
        if let Some(agent) = role_agent(conn, run_id, &room_id, r)? {
            scheduler::enqueue(conn, run_id, EnqueueSpec {
                agent_id: &agent,
                room_id: &room_id,
                trigger_event_id: &p.event_id,
                trigger_message_id: Some(&p.message.message_id),
                priority: scheduler::PRIO_TASK,
                chain_depth: 0,
                reason: &format!("intent:{env_id}:RECONCILE"),
            })?;
        }
    }
    let _ = msg;
    Ok(())
}

pub fn list_envelopes(conn: &Connection, run_id: &str, room_id: &str, operator: bool) -> Result<Vec<Value>> {
    let mut st = conn.prepare("SELECT id, author_id, card_json, sealed, message_id, created_seq, revealed_seq FROM intent_envelopes WHERE run_id=?1 AND room_id=?2 ORDER BY created_seq DESC")?;
    let v = st
        .query_map(params![run_id, room_id], |r| {
            let sealed = r.get::<_, i64>(3)? != 0;
            let card: String = r.get(2)?;
            Ok(json!({
                "id": r.get::<_, String>(0)?, "author_id": r.get::<_, String>(1)?, "sealed": sealed,
                "card": if !sealed || operator { serde_json::from_str::<Value>(&card).unwrap_or_default() } else { Value::Null },
                "message_id": r.get::<_, Option<String>>(4)?, "created_seq": r.get::<_, i64>(5)?, "revealed_seq": r.get::<_, Option<i64>>(6)?
            }))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(v)
}
