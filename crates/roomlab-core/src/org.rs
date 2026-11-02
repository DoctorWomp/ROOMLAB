//! Guild org graph and task clusters (§4.1–4.3). Org edges are *organizational* relations
//! only: they never grant RAG access, tool privileges or spawn authority (T35).

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::auth;
use crate::contracts::*;
use crate::error::{reject, CoreError, Result};
use crate::events::{self, NewEvent};
use crate::ids::new_id;

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct OrgEdge {
    pub id: String,
    pub room_id: String,
    pub from_id: String,
    pub to_id: String,
    pub relation: OrgRelation,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct TaskCluster {
    pub cluster_id: String,
    pub room_id: String,
    pub goal: String,
    pub owner_agent_id: String,
    pub status: TaskStatus,
    pub assignees: Vec<String>,
    pub priority: i64,
    pub dependencies: Vec<String>,
    pub verification_criteria: Vec<String>,
    pub human_action_needed: bool,
    pub notes: Vec<serde_json::Value>,
}

pub fn add_edge(conn: &Connection, run_id: &str, room_id: &str, from: &str, to: &str, relation: OrgRelation, by: &str) -> Result<String> {
    auth::require_member(conn, run_id, room_id, from)?;
    auth::require_member(conn, run_id, room_id, to)?;
    if from == to {
        return Err(CoreError::Invalid("an identity cannot relate to itself".into()));
    }
    let dup: Option<String> = conn
        .query_row(
            "SELECT id FROM org_edges WHERE run_id=?1 AND room_id=?2 AND from_id=?3 AND to_id=?4 AND relation=?5",
            params![run_id, room_id, from, to, relation.as_str()],
            |r| r.get(0),
        )
        .optional()?;
    if let Some(d) = dup {
        return Ok(d);
    }
    let id = new_id("org");
    conn.execute(
        "INSERT INTO org_edges(run_id, id, room_id, from_id, to_id, relation) VALUES (?1,?2,?3,?4,?5,?6)",
        params![run_id, id, room_id, from, to, relation.as_str()],
    )?;
    events::append(
        conn,
        run_id,
        NewEvent::new(EventType::OrgChanged, OriginKind::Human, by, AudienceScope::RoomMembers, json!({"edge_id": id, "from": from, "to": to, "relation": relation, "added": true})).room(room_id),
    )?;
    Ok(id)
}

pub fn remove_edge(conn: &Connection, run_id: &str, edge_id: &str, by: &str) -> Result<()> {
    let room: Option<String> = conn.query_row("SELECT room_id FROM org_edges WHERE run_id=?1 AND id=?2", params![run_id, edge_id], |r| r.get(0)).optional()?;
    let Some(room) = room else { return Err(CoreError::NotFound(format!("edge {edge_id}"))) };
    conn.execute("DELETE FROM org_edges WHERE run_id=?1 AND id=?2", params![run_id, edge_id])?;
    events::append(
        conn,
        run_id,
        NewEvent::new(EventType::OrgChanged, OriginKind::Human, by, AudienceScope::RoomMembers, json!({"edge_id": edge_id, "added": false})).room(&room),
    )?;
    Ok(())
}

pub fn edges(conn: &Connection, run_id: &str, room_id: &str) -> Result<Vec<OrgEdge>> {
    let mut st = conn.prepare("SELECT id, room_id, from_id, to_id, relation FROM org_edges WHERE run_id=?1 AND room_id=?2 ORDER BY relation, from_id")?;
    let rows = st
        .query_map(params![run_id, room_id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, String>(3)?, r.get::<_, String>(4)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    rows.into_iter()
        .map(|(id, room_id, from_id, to_id, rel)| Ok(OrgEdge { id, room_id, from_id, to_id, relation: parse_enum(&rel)? }))
        .collect()
}

pub fn has_edge(conn: &Connection, run_id: &str, room_id: &str, from: &str, relation: OrgRelation) -> Result<bool> {
    Ok(conn
        .query_row(
            "SELECT 1 FROM org_edges WHERE run_id=?1 AND room_id=?2 AND from_id=?3 AND relation=?4 LIMIT 1",
            params![run_id, room_id, from, relation.as_str()],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}

pub fn next_cluster_id(conn: &Connection, run_id: &str) -> Result<String> {
    let n: i64 = conn.query_row("SELECT COUNT(*) FROM tasks WHERE run_id=?1", params![run_id], |r| r.get(0))?;
    Ok(format!("CASE-{:02}", n + 1))
}

pub fn create_task(conn: &Connection, run_id: &str, t: &TaskCluster, by: &str) -> Result<()> {
    if t.goal.trim().is_empty() || t.goal.chars().count() > 1000 {
        return Err(CoreError::Invalid("goal must be 1–1000 characters".into()));
    }
    for a in &t.assignees {
        auth::require_member(conn, run_id, &t.room_id, a)?;
    }
    let seq = events::current_seq(conn, run_id)?;
    conn.execute(
        "INSERT INTO tasks(run_id, cluster_id, room_id, goal, owner_agent_id, status, assignees_json, priority, dependencies_json, verification_criteria_json, human_action_needed, notes_json, updated_seq)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,'[]',?12)",
        params![
            run_id, t.cluster_id, t.room_id, t.goal, t.owner_agent_id, t.status.as_str(), serde_json::to_string(&t.assignees)?,
            t.priority, serde_json::to_string(&t.dependencies)?, serde_json::to_string(&t.verification_criteria)?, t.human_action_needed as i64, seq
        ],
    )?;
    for a in &t.assignees {
        add_edge(conn, run_id, &t.room_id, &t.owner_agent_id, a, OrgRelation::AssignedTask, by).ok();
    }
    events::append(
        conn,
        run_id,
        NewEvent::new(EventType::TaskUpdated, OriginKind::Agent, by, AudienceScope::RoomMembers, json!({"cluster_id": t.cluster_id, "status": t.status, "created": true, "assignees": t.assignees})).room(&t.room_id),
    )?;
    Ok(())
}

pub fn get_task(conn: &Connection, run_id: &str, cluster_id: &str) -> Result<TaskCluster> {
    let r = conn
        .query_row(
            "SELECT cluster_id, room_id, goal, owner_agent_id, status, assignees_json, priority, dependencies_json, verification_criteria_json, human_action_needed, notes_json
             FROM tasks WHERE run_id=?1 AND cluster_id=?2",
            params![run_id, cluster_id],
            |r| {
                Ok((
                    r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, String>(3)?, r.get::<_, String>(4)?,
                    r.get::<_, String>(5)?, r.get::<_, i64>(6)?, r.get::<_, String>(7)?, r.get::<_, String>(8)?, r.get::<_, i64>(9)?, r.get::<_, String>(10)?,
                ))
            },
        )
        .optional()?
        .ok_or_else(|| CoreError::Rejected { code: ReasonCode::TargetUnknown, message: format!("no task {cluster_id}") })?;
    Ok(TaskCluster {
        cluster_id: r.0,
        room_id: r.1,
        goal: r.2,
        owner_agent_id: r.3,
        status: parse_enum(&r.4)?,
        assignees: serde_json::from_str(&r.5)?,
        priority: r.6,
        dependencies: serde_json::from_str(&r.7)?,
        verification_criteria: serde_json::from_str(&r.8)?,
        human_action_needed: r.9 != 0,
        notes: serde_json::from_str(&r.10)?,
    })
}

pub fn list_tasks(conn: &Connection, run_id: &str, room_id: &str) -> Result<Vec<TaskCluster>> {
    let mut st = conn.prepare("SELECT cluster_id FROM tasks WHERE run_id=?1 AND room_id=?2 ORDER BY cluster_id")?;
    let ids = st.query_map(params![run_id, room_id], |r| r.get::<_, String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
    ids.iter().map(|i| get_task(conn, run_id, i)).collect()
}

/// Status update. Agents may update a task they own or are assigned to; only a human may set VERIFIED.
pub fn update_task(conn: &Connection, run_id: &str, cluster_id: &str, actor: &str, actor_is_human: bool, status: Option<TaskStatus>, note: Option<&str>) -> Result<()> {
    let t = get_task(conn, run_id, cluster_id)?;
    if !actor_is_human && t.owner_agent_id != actor && !t.assignees.iter().any(|a| a == actor) {
        return reject(ReasonCode::CapabilityDenied, "only the owner or assignees may update this task");
    }
    if status == Some(TaskStatus::Verified) && !actor_is_human {
        return reject(ReasonCode::CapabilityDenied, "only the human operator verifies a task");
    }
    let seq = events::current_seq(conn, run_id)?;
    if let Some(s) = status {
        conn.execute(
            "UPDATE tasks SET status=?3, updated_seq=?4, human_action_needed=?5 WHERE run_id=?1 AND cluster_id=?2",
            params![run_id, cluster_id, s.as_str(), seq, (s == TaskStatus::ReadyForReview) as i64],
        )?;
    }
    if let Some(n) = note {
        let mut notes = t.notes.clone();
        notes.push(json!({"by": actor, "note": n.chars().take(500).collect::<String>(), "seq": seq}));
        conn.execute("UPDATE tasks SET notes_json=?3 WHERE run_id=?1 AND cluster_id=?2", params![run_id, cluster_id, serde_json::to_string(&notes)?])?;
    }
    events::append(
        conn,
        run_id,
        NewEvent::new(EventType::TaskUpdated, if actor_is_human { OriginKind::Human } else { OriginKind::Agent }, actor, AudienceScope::RoomMembers, json!({
            "cluster_id": cluster_id, "status": status, "note": note.is_some()
        }))
        .room(&t.room_id),
    )?;
    Ok(())
}
