//! Rooms, channels and the communication graph (membership) — §4.

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::contracts::*;
use crate::error::{reject, CoreError, Result};
use crate::events::{self, NewEvent};
use crate::ids::{new_id, now_utc};

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct RoomSettings {
    pub spawn_policy: SpawnPolicy,
    pub max_active_children: i64,
    pub max_children_per_parent: i64,
    pub child_max_turns: i64,
    pub child_ttl_seconds: i64,
    pub child_max_output_tokens: i64,
    /// Plain prose without JSON is accepted as SPEAK (lowest privilege action).
    pub plain_text_as_speak: bool,
    pub narrative_resolution: NarrativeResolution,
    /// QUEUE_FOR_PROVIDER | ASK_HUMAN | DECLINE — never an undisclosed substitution.
    pub unavailable_policy: String,
    pub org_mode: OrgMode,
    pub default_model_route: String,
    pub soma_enabled: bool,
    pub read_only: bool,
}

impl Default for RoomSettings {
    fn default() -> Self {
        RoomSettings {
            spawn_policy: SpawnPolicy::ApprovalRequired,
            max_active_children: 3,
            max_children_per_parent: 2,
            child_max_turns: 8,
            child_ttl_seconds: 300,
            child_max_output_tokens: 1500,
            plain_text_as_speak: true,
            narrative_resolution: NarrativeResolution::Balanced,
            unavailable_policy: "DECLINE".into(),
            org_mode: OrgMode::Manual,
            default_model_route: "demo".into(),
            soma_enabled: true,
            read_only: false,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct RoomInfo {
    pub id: String,
    pub kind: RoomKind,
    pub name: String,
    pub settings: RoomSettings,
    pub rule_revision: i64,
    pub members: Vec<MemberInfo>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct MemberInfo {
    pub agent_id: String,
    pub display_name: String,
    pub icon: String,
    pub kind: String,
    pub role: String,
    pub status: String,
    pub post_scope: String,
    pub cognition: String,
    pub model_route: String,
    pub parent_agent_id: Option<String>,
}

pub fn create_room(conn: &Connection, run_id: &str, kind: RoomKind, name: &str, settings: &RoomSettings, id: Option<String>) -> Result<String> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 80 {
        return Err(CoreError::Invalid("room name must be 1–80 characters".into()));
    }
    let id = id.unwrap_or_else(|| new_id(&kind.as_str().to_lowercase()));
    conn.execute(
        "INSERT INTO rooms(run_id, id, kind, name, settings_json, rule_revision, created_at) VALUES (?1,?2,?3,?4,?5,1,?6)",
        params![run_id, id, kind.as_str(), name, serde_json::to_string(settings)?, now_utc()],
    )?;
    for (cid, cname, ckind) in [("public", "public", "PUBLIC"), ("narrator", "narrator", "NARRATOR")] {
        conn.execute(
            "INSERT INTO channels(run_id, id, room_id, name, kind) VALUES (?1,?2,?3,?4,?5)",
            params![run_id, format!("{id}:{cid}"), id, cname, ckind],
        )?;
    }
    events::append(
        conn,
        run_id,
        NewEvent::new(EventType::RoomCreated, OriginKind::Human, crate::HUMAN_ID, AudienceScope::RoomMembers, json!({"room_id": id, "kind": kind, "name": name})).room(&id),
    )?;
    Ok(id)
}

pub fn room_kind(conn: &Connection, run_id: &str, room_id: &str) -> Result<RoomKind> {
    let k: Option<String> = conn
        .query_row("SELECT kind FROM rooms WHERE run_id=?1 AND id=?2", params![run_id, room_id], |r| r.get(0))
        .optional()?;
    match k {
        Some(k) => parse_enum(&k),
        None => reject(ReasonCode::TargetUnknown, format!("unknown room {room_id}")),
    }
}

pub fn room_settings(conn: &Connection, run_id: &str, room_id: &str) -> Result<RoomSettings> {
    let s: Option<String> = conn
        .query_row("SELECT settings_json FROM rooms WHERE run_id=?1 AND id=?2", params![run_id, room_id], |r| r.get(0))
        .optional()?;
    match s {
        Some(s) => Ok(serde_json::from_str(&s)?),
        None => reject(ReasonCode::TargetUnknown, format!("unknown room {room_id}")),
    }
}

pub fn set_room_settings(conn: &Connection, run_id: &str, room_id: &str, s: &RoomSettings) -> Result<()> {
    conn.execute(
        "UPDATE rooms SET settings_json=?3, rule_revision = rule_revision + 1 WHERE run_id=?1 AND id=?2",
        params![run_id, room_id, serde_json::to_string(s)?],
    )?;
    Ok(())
}

pub fn add_member(conn: &Connection, run_id: &str, room_id: &str, agent_id: &str, role: &str, post_scope: &str) -> Result<()> {
    let seq = events::current_seq(conn, run_id)?;
    conn.execute(
        "INSERT INTO memberships(run_id, room_id, agent_id, role, can_post, post_scope, joined_seq) VALUES (?1,?2,?3,?4,1,?5,?6)
         ON CONFLICT(run_id, room_id, agent_id) DO UPDATE SET role=excluded.role, post_scope=excluded.post_scope",
        params![run_id, room_id, agent_id, role, post_scope, seq],
    )?;
    events::append(
        conn,
        run_id,
        NewEvent::new(EventType::MembershipChanged, OriginKind::System, "roomlab", AudienceScope::RoomMembers, json!({"agent_id": agent_id, "role": role, "joined": true, "post_scope": post_scope})).room(room_id),
    )?;
    Ok(())
}

pub fn remove_member(conn: &Connection, run_id: &str, room_id: &str, agent_id: &str) -> Result<()> {
    let n = conn.execute(
        "DELETE FROM memberships WHERE run_id=?1 AND room_id=?2 AND agent_id=?3",
        params![run_id, room_id, agent_id],
    )?;
    if n > 0 {
        events::append(
            conn,
            run_id,
            NewEvent::new(EventType::MembershipChanged, OriginKind::System, "roomlab", AudienceScope::RoomMembers, json!({"agent_id": agent_id, "joined": false})).room(room_id),
        )?;
    }
    Ok(())
}

pub fn member_ids(conn: &Connection, run_id: &str, room_id: &str) -> Result<Vec<String>> {
    let mut st = conn.prepare("SELECT agent_id FROM memberships WHERE run_id=?1 AND room_id=?2 ORDER BY joined_seq, agent_id")?;
    let v = st.query_map(params![run_id, room_id], |r| r.get(0))?.collect::<rusqlite::Result<Vec<String>>>()?;
    Ok(v)
}

pub fn members(conn: &Connection, run_id: &str, room_id: &str) -> Result<Vec<MemberInfo>> {
    let mut st = conn.prepare(
        "SELECT a.id, a.display_name, a.icon, a.kind, m.role, a.status, m.post_scope, a.cognition, a.model_route, a.parent_agent_id
         FROM memberships m JOIN agents a ON a.run_id=m.run_id AND a.id=m.agent_id
         WHERE m.run_id=?1 AND m.room_id=?2 ORDER BY a.kind='HUMAN' DESC, a.parent_agent_id IS NOT NULL, m.joined_seq, a.display_name",
    )?;
    let v = st
        .query_map(params![run_id, room_id], |r| {
            Ok(MemberInfo {
                agent_id: r.get(0)?,
                display_name: r.get(1)?,
                icon: r.get(2)?,
                kind: r.get(3)?,
                role: r.get(4)?,
                status: r.get(5)?,
                post_scope: r.get(6)?,
                cognition: r.get(7)?,
                model_route: r.get(8)?,
                parent_agent_id: r.get(9)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(v)
}

pub fn list_rooms(conn: &Connection, run_id: &str) -> Result<Vec<RoomInfo>> {
    let mut st = conn.prepare("SELECT id, kind, name, settings_json, rule_revision FROM rooms WHERE run_id=?1 AND archived=0 ORDER BY created_at, id")?;
    let rows = st
        .query_map(params![run_id], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, String>(3)?, r.get::<_, i64>(4)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut out = vec![];
    for (id, kind, name, sj, rev) in rows {
        out.push(RoomInfo {
            members: members(conn, run_id, &id)?,
            id,
            kind: parse_enum(&kind)?,
            name,
            settings: serde_json::from_str(&sj)?,
            rule_revision: rev,
        });
    }
    Ok(out)
}

pub fn get_room(conn: &Connection, run_id: &str, room_id: &str) -> Result<RoomInfo> {
    list_rooms(conn, run_id)?
        .into_iter()
        .find(|r| r.id == room_id)
        .ok_or_else(|| CoreError::NotFound(format!("room {room_id}")))
}

pub fn rooms_of_agent(conn: &Connection, run_id: &str, agent_id: &str) -> Result<Vec<String>> {
    let mut st = conn.prepare("SELECT room_id FROM memberships WHERE run_id=?1 AND agent_id=?2")?;
    let v = st.query_map(params![run_id, agent_id], |r| r.get(0))?.collect::<rusqlite::Result<Vec<String>>>()?;
    Ok(v)
}
