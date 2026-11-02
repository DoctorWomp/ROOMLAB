//! Persistent agent identity (§5). Identity is a DB record, not a process or loaded model.

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::auth;
use crate::contracts::*;
use crate::error::{reject, CoreError, Result};
use crate::events::{self, NewEvent};
use crate::ids::{new_id, now_utc};

pub const STORY_ACTIONS: &[ActionKind] = &[
    ActionKind::Look, ActionKind::Inspect, ActionKind::Move, ActionKind::Speak, ActionKind::Whisper,
    ActionKind::Take, ActionKind::Drop, ActionKind::Use, ActionKind::Wait, ActionKind::Ask, ActionKind::NoOp,
];
pub const CHAT_ACTIONS: &[ActionKind] = &[ActionKind::Speak, ActionKind::Ask, ActionKind::Whisper, ActionKind::NoOp, ActionKind::Wait, ActionKind::Tool];

pub struct ProfileSeed<'a> {
    pub id: &'a str,
    pub name: &'a str,
    pub icon: &'a str,
    pub role: &'a str,
    pub persona: &'a str,
    pub directive: &'a str,
    pub cognition: Cognition,
    pub route: &'a str,
    pub soma: SomaMode,
    pub demo_policy: Option<&'a str>,
    pub actions: Vec<ActionKind>,
    pub motivation: &'a str,
    pub style: &'a str,
}

pub fn profile_from_seed(s: &ProfileSeed) -> AgentProfile {
    AgentProfile {
        schema_version: SCHEMA_VERSION.into(),
        agent_id: s.id.into(),
        display_name: s.name.into(),
        icon: s.icon.into(),
        model_route: s.route.into(),
        cognition: s.cognition,
        soma_mode: s.soma,
        persona: s.persona.into(),
        directive: s.directive.into(),
        character: CharacterSpec {
            role: s.role.into(),
            motivation_core: s.motivation.into(),
            motivation_lock: "FLEXIBLE".into(),
            secondary_drives: vec![],
            role_rigidity: "SOFT".into(),
            description_detail: "SPARSE".into(),
            expressive_style: s.style.into(),
            social_axes: SocialAxes { sociability: 0.5, skepticism: 0.5, expressivity: 0.4 },
            belief_update_policy: "UPDATE_ON_EVIDENCE".into(),
            temperament_variance: 0.1,
        },
        knowledge: KnowledgeSpec {
            private_namespace: format!("agent/{}/private", s.id),
            readable_mounts: vec![format!("agent/{}/private", s.id)],
            initial_beliefs: vec![],
        },
        autonomy: AutonomySpec {
            policy: "EVENT_DRIVEN".into(),
            may_propose_new_goals: true,
            may_choose_silence: true,
            allowed_actions: s.actions.clone(),
        },
        limits: LimitsSpec { turn_output_tokens: 200, max_queued_calls: 1 },
        demo_policy: s.demo_policy.map(|d| d.to_string()),
    }
}

pub fn validate_profile(p: &AgentProfile) -> Result<()> {
    let bad = |m: &str| Err(CoreError::Invalid(m.to_string()));
    if p.schema_version != SCHEMA_VERSION {
        return bad("unsupported profile schema_version");
    }
    if p.agent_id.is_empty() || p.agent_id.len() > 48 || !p.agent_id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
        return bad("agent_id must be 1–48 chars of [A-Za-z0-9_-]");
    }
    let n = p.display_name.trim().chars().count();
    if n == 0 || n > 40 {
        return bad("display name must be 1–40 characters");
    }
    if p.icon.chars().count() > 8 {
        return bad("icon must be a short emoji or glyph");
    }
    if p.persona.len() > 8000 || p.directive.len() > 4000 {
        return bad("persona/directive too long");
    }
    if !(1..=2000).contains(&p.limits.turn_output_tokens) {
        return bad("turn_output_tokens must be 1–2000");
    }
    if p.cognition.uses_model() && p.model_route == "demo" {
        return bad("model-backed cognition requires a non-demo model route");
    }
    if !p.cognition.uses_model() && p.demo_policy.is_none() {
        return bad("L0 scripted agents need a demo policy");
    }
    Ok(())
}

pub fn create_agent(conn: &Connection, run_id: &str, p: &AgentProfile, kind: AgentKind, parent: Option<&str>, spawn_type: Option<SpawnType>) -> Result<()> {
    validate_profile(p)?;
    if auth::load_agent(conn, run_id, &p.agent_id)?.is_some() {
        return Err(CoreError::Invalid(format!("agent id '{}' already exists", p.agent_id)));
    }
    let route_ok: bool = conn
        .query_row("SELECT COUNT(*) FROM model_routes WHERE id=?1 AND enabled=1", params![p.model_route], |r| r.get::<_, i64>(0))?
        > 0;
    if !route_ok {
        return reject(ReasonCode::ModelUnavailable, format!("model route '{}' does not exist", p.model_route));
    }
    let now = now_utc();
    conn.execute(
        "INSERT INTO agents(run_id, id, display_name, icon, kind, cognition, model_route, status, parent_agent_id, spawn_type, soma_mode, demo_policy, created_at, updated_at)
         VALUES (?1,?2,?3,?4,?5,?6,?7,'ACTIVE',?8,?9,?10,?11,?12,?12)",
        params![
            run_id, p.agent_id, p.display_name.trim(), p.icon, kind.as_str(), p.cognition.as_str(), p.model_route,
            parent, spawn_type.map(|s| s.as_str()), p.soma_mode.as_str(), p.demo_policy, now
        ],
    )?;
    conn.execute(
        "INSERT INTO agent_profiles(run_id, agent_id, version, profile_json) VALUES (?1,?2,1,?3)",
        params![run_id, p.agent_id, serde_json::to_string(p)?],
    )?;
    conn.execute(
        "INSERT OR IGNORE INTO runtime_usage(run_id, agent_id) VALUES (?1, ?2)",
        params![run_id, p.agent_id],
    )?;
    let seq = events::current_seq(conn, run_id)?;
    for b in &p.knowledge.initial_beliefs {
        conn.execute(
            "INSERT INTO beliefs(run_id, id, agent_id, claim, status, source_ref, updated_seq) VALUES (?1,?2,?3,?4,?5,?6,?7)",
            params![run_id, new_id("bel"), p.agent_id, b.claim, b.status, b.source_ref, seq],
        )?;
    }
    events::append(
        conn,
        run_id,
        NewEvent::new(EventType::AgentCreated, OriginKind::System, "roomlab", AudienceScope::OperatorOnly, json!({
            "agent_id": p.agent_id, "display_name": p.display_name, "kind": kind, "cognition": p.cognition,
            "model_route": p.model_route, "parent_agent_id": parent, "spawn_type": spawn_type
        })),
    )?;
    Ok(())
}

/// Edit preserves the stable agent_id (T13); bumps the profile version.
pub fn update_agent(conn: &Connection, run_id: &str, p: &AgentProfile) -> Result<i64> {
    validate_profile(p)?;
    let existing = auth::require_agent(conn, run_id, &p.agent_id)?;
    let route_ok: bool = conn
        .query_row("SELECT COUNT(*) FROM model_routes WHERE id=?1 AND enabled=1", params![p.model_route], |r| r.get::<_, i64>(0))?
        > 0;
    if !route_ok {
        return reject(ReasonCode::ModelUnavailable, format!("model route '{}' does not exist", p.model_route));
    }
    conn.execute(
        "UPDATE agents SET display_name=?3, icon=?4, cognition=?5, model_route=?6, soma_mode=?7, demo_policy=?8, updated_at=?9 WHERE run_id=?1 AND id=?2",
        params![run_id, p.agent_id, p.display_name.trim(), p.icon, p.cognition.as_str(), p.model_route, p.soma_mode.as_str(), p.demo_policy, now_utc()],
    )?;
    let v: i64 = conn.query_row(
        "UPDATE agent_profiles SET version = version + 1, profile_json=?3 WHERE run_id=?1 AND agent_id=?2 RETURNING version",
        params![run_id, p.agent_id, serde_json::to_string(p)?],
        |r| r.get(0),
    )?;
    events::append(
        conn,
        run_id,
        NewEvent::new(EventType::AgentUpdated, OriginKind::Human, crate::HUMAN_ID, AudienceScope::OperatorOnly, json!({
            "agent_id": p.agent_id, "version": v, "previous_name": existing.display_name, "display_name": p.display_name,
            "cognition": p.cognition, "model_route": p.model_route, "soma_mode": p.soma_mode
        })),
    )?;
    Ok(v)
}

pub fn get_profile(conn: &Connection, run_id: &str, agent_id: &str) -> Result<AgentProfile> {
    let j: Option<String> = conn
        .query_row("SELECT profile_json FROM agent_profiles WHERE run_id=?1 AND agent_id=?2", params![run_id, agent_id], |r| r.get(0))
        .optional()?;
    match j {
        Some(j) => Ok(serde_json::from_str(&j)?),
        None => Err(CoreError::NotFound(format!("profile {agent_id}"))),
    }
}

pub fn set_status(conn: &Connection, run_id: &str, agent_id: &str, status: AgentStatus, reason: &str) -> Result<()> {
    let a = auth::require_agent(conn, run_id, agent_id)?;
    if a.status == status {
        return Ok(());
    }
    conn.execute(
        "UPDATE agents SET status=?3, updated_at=?4 WHERE run_id=?1 AND id=?2",
        params![run_id, agent_id, status.as_str(), now_utc()],
    )?;
    if status != AgentStatus::Active {
        // Dormant/archived identities hold no pending generation (T26).
        conn.execute(
            "UPDATE jobs SET status='CANCELLED', outcome='IDENTITY_NOT_ACTIVE', finished_wall_ms=?3 WHERE run_id=?1 AND agent_id=?2 AND status='QUEUED'",
            params![run_id, agent_id, crate::ids::now_ms()],
        )?;
    }
    events::append(
        conn,
        run_id,
        NewEvent::new(EventType::AgentStatusChanged, OriginKind::System, "roomlab", AudienceScope::OperatorOnly, json!({
            "agent_id": agent_id, "from": a.status, "to": status, "reason": reason
        })),
    )?;
    Ok(())
}

/// "Delete" archives the identity and removes memberships; history stays auditable.
pub fn delete_agent(conn: &Connection, run_id: &str, agent_id: &str) -> Result<()> {
    if agent_id == crate::HUMAN_ID {
        return Err(CoreError::Invalid("the human operator identity cannot be deleted".into()));
    }
    set_status(conn, run_id, agent_id, AgentStatus::Archived, "deleted by operator")?;
    for room in crate::room::rooms_of_agent(conn, run_id, agent_id)? {
        crate::room::remove_member(conn, run_id, &room, agent_id)?;
    }
    Ok(())
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct AgentSummary {
    pub id: String,
    pub display_name: String,
    pub icon: String,
    pub kind: String,
    pub cognition: String,
    pub model_route: String,
    pub status: String,
    pub soma_mode: String,
    pub parent_agent_id: Option<String>,
    pub spawn_type: Option<String>,
    pub role: String,
    pub queued_jobs: i64,
    pub running_jobs: i64,
    pub calls: i64,
    pub output_tokens: i64,
    pub demo_turns: i64,
}

pub fn list_agents(conn: &Connection, run_id: &str) -> Result<Vec<AgentSummary>> {
    let mut st = conn.prepare(
        "SELECT a.id, a.display_name, a.icon, a.kind, a.cognition, a.model_route, a.status, a.soma_mode, a.parent_agent_id, a.spawn_type,
           COALESCE(json_extract(p.profile_json,'$.character.role'),''),
           (SELECT COUNT(*) FROM jobs j WHERE j.run_id=a.run_id AND j.agent_id=a.id AND j.status='QUEUED'),
           (SELECT COUNT(*) FROM jobs j WHERE j.run_id=a.run_id AND j.agent_id=a.id AND j.status='RUNNING'),
           COALESCE(u.calls,0), COALESCE(u.output_tokens,0), COALESCE(u.demo_turns,0)
         FROM agents a LEFT JOIN agent_profiles p ON p.run_id=a.run_id AND p.agent_id=a.id
         LEFT JOIN runtime_usage u ON u.run_id=a.run_id AND u.agent_id=a.id
         WHERE a.run_id=?1 ORDER BY a.kind='HUMAN' DESC, a.created_at, a.id",
    )?;
    let v = st
        .query_map(params![run_id], |r| {
            Ok(AgentSummary {
                id: r.get(0)?,
                display_name: r.get(1)?,
                icon: r.get(2)?,
                kind: r.get(3)?,
                cognition: r.get(4)?,
                model_route: r.get(5)?,
                status: r.get(6)?,
                soma_mode: r.get(7)?,
                parent_agent_id: r.get(8)?,
                spawn_type: r.get(9)?,
                role: r.get(10)?,
                queued_jobs: r.get(11)?,
                running_jobs: r.get(12)?,
                calls: r.get(13)?,
                output_tokens: r.get(14)?,
                demo_turns: r.get(15)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(v)
}

pub fn ensure_human(conn: &Connection, run_id: &str) -> Result<()> {
    if auth::load_agent(conn, run_id, crate::HUMAN_ID)?.is_some() {
        return Ok(());
    }
    let p = profile_from_seed(&ProfileSeed {
        id: crate::HUMAN_ID,
        name: "Human",
        icon: "🧑",
        role: "operator",
        persona: "The human operator.",
        directive: "",
        cognition: Cognition::L0Scripted,
        route: "demo",
        soma: SomaMode::Light,
        demo_policy: Some("human"),
        actions: {
            let mut v = STORY_ACTIONS.to_vec();
            v.extend([ActionKind::Tool, ActionKind::AssignTask, ActionKind::UpdateTask]);
            v
        },
        motivation: "",
        style: "",
    });
    create_agent(conn, run_id, &p, AgentKind::Human, None, None)
}

pub fn display_name(conn: &Connection, run_id: &str, agent_id: &str) -> String {
    conn.query_row("SELECT display_name FROM agents WHERE run_id=?1 AND id=?2", params![run_id, agent_id], |r| r.get(0))
        .unwrap_or_else(|_| agent_id.to_string())
}
