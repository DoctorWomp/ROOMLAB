//! Principals and the permission matrix (§12.3). Role hierarchy, spawn parentage, chat
//! membership, RAG permissions and tool authority are separate relations (§2.3):
//! nothing here consults org rank.

use rusqlite::{params, Connection, OptionalExtension};

use crate::contracts::*;
use crate::error::{reject, Result};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Principal {
    /// Human operator with explicit OPERATOR TRUTH view.
    Operator,
    /// Any identity in the run (human participant, agent, narrator, world entity).
    Identity(String),
}

impl Principal {
    pub fn id(&self) -> &str {
        match self {
            Principal::Operator => "operator",
            Principal::Identity(id) => id,
        }
    }
}

#[derive(Clone, Debug)]
pub struct AgentRow {
    pub id: String,
    pub display_name: String,
    pub icon: String,
    pub kind: AgentKind,
    pub cognition: Cognition,
    pub model_route: String,
    pub status: AgentStatus,
    pub parent_agent_id: Option<String>,
    pub spawn_type: Option<String>,
    pub soma_mode: SomaMode,
    pub demo_policy: Option<String>,
}

pub fn load_agent(conn: &Connection, run_id: &str, agent_id: &str) -> Result<Option<AgentRow>> {
    let row = conn
        .query_row(
            "SELECT id, display_name, icon, kind, cognition, model_route, status, parent_agent_id, spawn_type, soma_mode, demo_policy
             FROM agents WHERE run_id=?1 AND id=?2",
            params![run_id, agent_id],
            |r| {
                Ok((
                    r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, String>(3)?,
                    r.get::<_, String>(4)?, r.get::<_, String>(5)?, r.get::<_, String>(6)?, r.get::<_, Option<String>>(7)?,
                    r.get::<_, Option<String>>(8)?, r.get::<_, String>(9)?, r.get::<_, Option<String>>(10)?,
                ))
            },
        )
        .optional()?;
    match row {
        None => Ok(None),
        Some((id, dn, icon, kind, cog, route, status, parent, st, soma, dp)) => Ok(Some(AgentRow {
            id,
            display_name: dn,
            icon,
            kind: parse_enum(&kind)?,
            cognition: parse_enum(&cog)?,
            model_route: route,
            status: parse_enum(&status)?,
            parent_agent_id: parent,
            spawn_type: st,
            soma_mode: parse_enum(&soma)?,
            demo_policy: dp,
        })),
    }
}

pub fn require_agent(conn: &Connection, run_id: &str, agent_id: &str) -> Result<AgentRow> {
    match load_agent(conn, run_id, agent_id)? {
        Some(a) => Ok(a),
        None => reject(ReasonCode::TargetUnknown, format!("unknown identity '{agent_id}'")),
    }
}

pub fn require_active(conn: &Connection, run_id: &str, agent_id: &str) -> Result<AgentRow> {
    let a = require_agent(conn, run_id, agent_id)?;
    if a.status != AgentStatus::Active {
        return reject(ReasonCode::ActorNotActive, format!("{} is {}", a.display_name, a.status.as_str()));
    }
    Ok(a)
}

#[derive(Clone, Debug)]
pub struct MembershipRow {
    pub role: String,
    pub can_post: bool,
    pub post_scope: String,
}

pub fn membership(conn: &Connection, run_id: &str, room_id: &str, agent_id: &str) -> Result<Option<MembershipRow>> {
    Ok(conn
        .query_row(
            "SELECT role, can_post, post_scope FROM memberships WHERE run_id=?1 AND room_id=?2 AND agent_id=?3",
            params![run_id, room_id, agent_id],
            |r| Ok(MembershipRow { role: r.get(0)?, can_post: r.get::<_, i64>(1)? != 0, post_scope: r.get(2)? }),
        )
        .optional()?)
}

pub fn require_member(conn: &Connection, run_id: &str, room_id: &str, agent_id: &str) -> Result<MembershipRow> {
    match membership(conn, run_id, room_id, agent_id)? {
        Some(m) => Ok(m),
        None => reject(ReasonCode::NotMember, format!("{agent_id} is not a member of {room_id}")),
    }
}

/// Is a child (spawned identity) — task children are depth 1 and may not spawn (M).
pub fn is_child(a: &AgentRow) -> bool {
    a.parent_agent_id.is_some()
}

/// Explicit allowed-action list from the agent's profile.
pub fn allowed_actions(conn: &Connection, run_id: &str, agent_id: &str) -> Result<Vec<ActionKind>> {
    let pj: Option<String> = conn
        .query_row(
            "SELECT profile_json FROM agent_profiles WHERE run_id=?1 AND agent_id=?2",
            params![run_id, agent_id],
            |r| r.get(0),
        )
        .optional()?;
    match pj {
        Some(j) => {
            let p: AgentProfile = serde_json::from_str(&j)?;
            Ok(p.autonomy.allowed_actions)
        }
        None => Ok(vec![ActionKind::Speak, ActionKind::NoOp, ActionKind::Wait]),
    }
}

pub fn require_capability(conn: &Connection, run_id: &str, agent_id: &str, action: ActionKind) -> Result<()> {
    // NO_OP and WAIT are always permitted: silence is a valid outcome (§8.3).
    if matches!(action, ActionKind::NoOp | ActionKind::Wait) {
        return Ok(());
    }
    let allowed = allowed_actions(conn, run_id, agent_id)?;
    if allowed.contains(&action) {
        Ok(())
    } else {
        reject(ReasonCode::CapabilityDenied, format!("action {} not in {agent_id}'s allowed actions", action.as_str()))
    }
}
