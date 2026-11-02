//! Authoritative append-only event log (§13.1–13.2). `seq` is allocated transactionally
//! per run inside the caller's transaction, so a rolled-back mutation never consumes a seq.

use rusqlite::{params, Connection, OptionalExtension, Row};
use serde_json::Value;

use crate::contracts::*;
use crate::error::{CoreError, Result};
use crate::ids::now_utc;

pub struct NewEvent {
    pub room_id: Option<String>,
    pub event_type: EventType,
    pub origin: Origin,
    pub audience: AudienceScope,
    pub payload: Value,
    pub parents: Vec<String>,
}

impl NewEvent {
    pub fn new(event_type: EventType, origin_kind: OriginKind, origin_id: &str, audience: AudienceScope, payload: Value) -> Self {
        NewEvent {
            room_id: None,
            event_type,
            origin: Origin { kind: origin_kind, id: origin_id.to_string() },
            audience,
            payload,
            parents: vec![],
        }
    }
    pub fn room(mut self, room_id: &str) -> Self {
        self.room_id = Some(room_id.to_string());
        self
    }
    pub fn parents(mut self, parents: Vec<String>) -> Self {
        self.parents = parents;
        self
    }
    pub fn system(event_type: EventType, payload: Value) -> Self {
        Self::new(event_type, OriginKind::System, "roomlab", AudienceScope::OperatorOnly, payload)
    }
}

pub fn event_id(run_id: &str, seq: i64) -> String {
    format!("evt_{}_{:06}", run_id.trim_start_matches("run_"), seq)
}

pub fn append(conn: &Connection, run_id: &str, ev: NewEvent) -> Result<EventEnvelope> {
    let (seq, sim): (i64, i64) = conn
        .query_row(
            "UPDATE experiment_runs SET next_seq = next_seq + 1 WHERE id = ?1 RETURNING next_seq - 1, sim_time_ms",
            params![run_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?
        .ok_or_else(|| CoreError::NotFound(format!("run {run_id}")))?;
    let env = EventEnvelope {
        schema_version: SCHEMA_VERSION.into(),
        event_id: event_id(run_id, seq),
        run_id: run_id.into(),
        room_id: ev.room_id,
        seq,
        sim_time_ms: sim,
        wall_time_utc: now_utc(),
        event_type: ev.event_type,
        origin: ev.origin,
        audience_scope: ev.audience,
        payload: ev.payload,
        parent_event_ids: ev.parents,
    };
    conn.execute(
        "INSERT INTO events (run_id, event_id, seq, room_id, sim_time_ms, wall_time_utc, event_type, origin_kind, origin_id, audience_scope, payload_json, parent_event_ids_json, schema_version)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)",
        params![
            env.run_id, env.event_id, env.seq, env.room_id, env.sim_time_ms, env.wall_time_utc,
            env.event_type.as_str(), env.origin.kind.as_str(), env.origin.id, env.audience_scope.as_str(),
            serde_json::to_string(&env.payload)?, serde_json::to_string(&env.parent_event_ids)?, env.schema_version
        ],
    )?;
    Ok(env)
}

/// Advance the simulation clock (independent from wall time).
pub fn advance_sim_clock(conn: &Connection, run_id: &str, ms: i64) -> Result<i64> {
    Ok(conn.query_row(
        "UPDATE experiment_runs SET sim_time_ms = sim_time_ms + ?2 WHERE id = ?1 RETURNING sim_time_ms",
        params![run_id, ms],
        |r| r.get(0),
    )?)
}

pub fn current_seq(conn: &Connection, run_id: &str) -> Result<i64> {
    Ok(conn.query_row("SELECT next_seq - 1 FROM experiment_runs WHERE id = ?1", params![run_id], |r| r.get(0))?)
}

pub fn sim_time(conn: &Connection, run_id: &str) -> Result<i64> {
    Ok(conn.query_row("SELECT sim_time_ms FROM experiment_runs WHERE id = ?1", params![run_id], |r| r.get(0))?)
}

fn row_to_event(r: &Row) -> rusqlite::Result<(String, String, String, String, String, String, i64, Option<String>, i64, String, String, String, String)> {
    Ok((
        r.get("run_id")?, r.get("event_id")?, r.get("event_type")?, r.get("origin_kind")?, r.get("origin_id")?,
        r.get("audience_scope")?, r.get("seq")?, r.get("room_id")?, r.get("sim_time_ms")?, r.get("wall_time_utc")?,
        r.get("payload_json")?, r.get("parent_event_ids_json")?, r.get("schema_version")?,
    ))
}

pub struct EventFilter {
    pub room_id: Option<String>,
    pub event_types: Vec<String>,
    pub origin_id: Option<String>,
    pub after_seq: i64,
    pub upto_seq: Option<i64>,
    pub limit: i64,
}

impl Default for EventFilter {
    fn default() -> Self {
        EventFilter { room_id: None, event_types: vec![], origin_id: None, after_seq: 0, upto_seq: None, limit: 500 }
    }
}

pub fn list(conn: &Connection, run_id: &str, f: &EventFilter) -> Result<Vec<EventEnvelope>> {
    let mut stmt = conn.prepare(
        "SELECT * FROM events WHERE run_id = ?1 AND seq > ?2 AND (?3 IS NULL OR seq <= ?3)
         AND (?4 IS NULL OR room_id = ?4) AND (?5 IS NULL OR origin_id = ?5)
         ORDER BY seq ASC LIMIT ?6",
    )?;
    let rows = stmt.query_map(
        params![run_id, f.after_seq, f.upto_seq, f.room_id, f.origin_id, f.limit.clamp(1, 20000)],
        row_to_event,
    )?;
    let mut out = vec![];
    for row in rows {
        let (run_id, event_id, et, ok, oid, aud, seq, room_id, sim, wall, payload, parents, sv) = row?;
        if !f.event_types.is_empty() && !f.event_types.contains(&et) {
            continue;
        }
        out.push(EventEnvelope {
            schema_version: sv,
            event_id,
            run_id,
            room_id,
            seq,
            sim_time_ms: sim,
            wall_time_utc: wall,
            event_type: parse_enum(&et)?,
            origin: Origin { kind: parse_enum(&ok)?, id: oid },
            audience_scope: parse_enum(&aud)?,
            payload: serde_json::from_str(&payload)?,
            parent_event_ids: serde_json::from_str(&parents)?,
        });
    }
    Ok(out)
}
