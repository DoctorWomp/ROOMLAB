//! Spawn Governor (§7). A spawn is a *request*, not a grant. Effective capabilities and budgets
//! are the intersection of actor authority, room policy, resource ACL and admin caps.
//! Every lifecycle transition is persisted; slot counts are derived from DB rows inside the
//! same transaction, so a crash cannot double-allocate.

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::agent::{self, ProfileSeed};
use crate::auth::{self, Principal};
use crate::comms::{self, PostSpec};
use crate::contracts::*;
use crate::error::{reject, CoreError, Result};
use crate::events::{self, NewEvent};
use crate::ids::{now_ms, slug};
use crate::knowledge;
use crate::room;
use crate::scheduler;

/// Actions a TASK_WORKER may ever hold (M). `REQUEST_SPAWN` is excluded: depth is capped at 1.
pub const TASK_WORKER_ACTIONS: &[ActionKind] = &[
    ActionKind::Speak, ActionKind::Tool, ActionKind::ReportProgress, ActionKind::ReportFinal,
    ActionKind::AskParent, ActionKind::NoOp, ActionKind::Wait,
];
pub const MAX_DEPTH: i64 = 1;
pub const MAX_HANDOFF_REFS: usize = 5;

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct SpawnDecision {
    pub request_id: String,
    pub state: SpawnState,
    pub reason_code: ReasonCode,
    pub child_agent_id: Option<String>,
    pub detail: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Effective {
    pub max_turns: i64,
    pub max_output_tokens_total: i64,
    pub ttl_seconds: i64,
    pub actions: Vec<ActionKind>,
    pub model_route: String,
    pub handoff_refs: Vec<String>,
}

fn transition(conn: &Connection, run_id: &str, request_id: &str, room_id: &str, from: Option<SpawnState>, to: SpawnState, reason: ReasonCode, detail: &str) -> Result<()> {
    let ev = events::append(
        conn,
        run_id,
        NewEvent::new(EventType::SpawnStateChanged, OriginKind::System, "spawn_governor", AudienceScope::OperatorOnly, json!({
            "request_id": request_id, "from": from, "to": to, "reason_code": reason, "detail": detail
        }))
        .room(room_id),
    )?;
    conn.execute(
        "UPDATE spawn_requests SET state=?3, reason_code=?4, updated_seq=?5 WHERE run_id=?1 AND request_id=?2",
        params![run_id, request_id, to.as_str(), reason.as_str(), ev.seq],
    )?;
    Ok(())
}

fn active_children_in_room(conn: &Connection, run_id: &str, room_id: &str) -> Result<i64> {
    Ok(conn.query_row(
        "SELECT COUNT(*) FROM spawn_instances WHERE run_id=?1 AND room_id=?2 AND state IN ('INITIALIZING','ACTIVE','SUSPENDED') AND request_id NOT LIKE 'summon_%'",
        params![run_id, room_id],
        |r| r.get(0),
    )?)
}

fn children_of(conn: &Connection, run_id: &str, parent: &str) -> Result<i64> {
    // Active children plus those already approved/queued for this parent.
    Ok(conn.query_row(
        "SELECT (SELECT COUNT(*) FROM spawn_instances WHERE run_id=?1 AND parent_agent_id=?2 AND state IN ('INITIALIZING','ACTIVE','SUSPENDED') AND request_id NOT LIKE 'summon_%')
              + (SELECT COUNT(*) FROM spawn_requests WHERE run_id=?1 AND parent_agent_id=?2 AND state IN ('AWAIT_APPROVAL','QUEUED'))",
        params![run_id, parent],
        |r| r.get(0),
    )?)
}

/// Validate and record a spawn request. Rejections are returned as `Rejected` errors so the
/// caller's transaction rolls back and the rejection is journaled separately.
pub fn request(conn: &Connection, run_id: &str, room_id: &str, req: &SpawnRequest, by_operator: bool) -> Result<SpawnDecision> {
    if req.schema_version != SCHEMA_VERSION || req.run_id != run_id {
        return reject(ReasonCode::InvalidSchema, "schema_version or run_id mismatch");
    }
    if req.request_id.is_empty() || req.request_id.len() > 64 {
        return reject(ReasonCode::InvalidSchema, "request_id required (≤64 chars)");
    }
    if conn.query_row("SELECT 1 FROM spawn_requests WHERE run_id=?1 AND request_id=?2", params![run_id, req.request_id], |_| Ok(())).optional()?.is_some() {
        return reject(ReasonCode::InvalidSchema, "duplicate request_id");
    }
    let parent = auth::require_active(conn, run_id, &req.parent_agent_id)?;
    auth::require_member(conn, run_id, room_id, &parent.id)?;
    // Depth: a child can never spawn in M (no grandchildren, no recursive parent-as-root).
    if auth::is_child(&parent) {
        return reject(ReasonCode::SpawnDepthExceeded, format!("max child depth is {MAX_DEPTH}"));
    }
    if req.spawn_type != SpawnType::TaskWorker && !by_operator {
        return reject(ReasonCode::CapabilityDenied, "agents may only request TASK_WORKER children in this beta");
    }
    if !by_operator {
        // Capability is an explicit grant, independent of org rank (T35).
        auth::require_capability(conn, run_id, &parent.id, ActionKind::RequestSpawn)?;
    }
    let settings = room::room_settings(conn, run_id, room_id)?;
    if settings.spawn_policy == SpawnPolicy::Disabled {
        return reject(ReasonCode::CapabilityDenied, "spawning is disabled in this room");
    }
    let name = req.display_name.trim();
    if name.is_empty() || name.chars().count() > 40 {
        return reject(ReasonCode::InvalidSchema, "display_name must be 1–40 characters");
    }
    if req.objective.trim().is_empty() || req.objective.chars().count() > 1500 {
        return reject(ReasonCode::InvalidSchema, "objective must be 1–1500 characters");
    }
    // Route must exist; the sub-agent route inherits the parent's route (no silent substitution).
    let route = if req.requested_model_route == "room_default_subagent" { parent.model_route.clone() } else { req.requested_model_route.clone() };
    let route_ok: bool = conn.query_row("SELECT COUNT(*) FROM model_routes WHERE id=?1 AND enabled=1", params![route], |r| r.get::<_, i64>(0))? > 0;
    if !route_ok {
        return reject(ReasonCode::ModelUnavailable, format!("model route '{}' unavailable", req.requested_model_route));
    }
    for a in &req.requested_actions {
        if !TASK_WORKER_ACTIONS.contains(a) {
            return reject(ReasonCode::CapabilityDenied, format!("action {} not grantable to a task worker", a.as_str()));
        }
    }
    if req.context_handoff_refs.len() > MAX_HANDOFF_REFS {
        return reject(ReasonCode::RateLimited, format!("at most {MAX_HANDOFF_REFS} handoff refs"));
    }
    for r in &req.context_handoff_refs {
        // The parent can only hand off what it can read itself.
        if !knowledge::can_read(conn, run_id, &Principal::Identity(parent.id.clone()), r)? {
            return reject(ReasonCode::AclDenied, format!("handoff ref '{r}' is not readable by the parent"));
        }
    }
    let b = &req.requested_budget;
    if b.max_turns <= 0 || b.max_output_tokens_total <= 0 || b.ttl_seconds <= 0 {
        return reject(ReasonCode::InvalidSchema, "budgets must be positive");
    }
    let run_b = scheduler::budget_state(conn, run_id)?;
    let model_backed = route != "demo";
    if model_backed && run_b.tokens_left() < 50 {
        return reject(ReasonCode::BudgetExhausted, "run output-token budget exhausted");
    }
    if run_b.turns_left() < 2 {
        return reject(ReasonCode::BudgetExhausted, "run turn budget exhausted");
    }
    if children_of(conn, run_id, &parent.id)? >= settings.max_children_per_parent {
        return reject(ReasonCode::SpawnCapReached, format!("{} already has {} children (cap)", parent.display_name, settings.max_children_per_parent));
    }
    let mut actions = req.requested_actions.clone();
    for must in [ActionKind::ReportFinal, ActionKind::NoOp] {
        if !actions.contains(&must) {
            actions.push(must);
        }
    }
    let eff = Effective {
        max_turns: b.max_turns.min(settings.child_max_turns),
        max_output_tokens_total: b.max_output_tokens_total.min(settings.child_max_output_tokens).min(if model_backed { run_b.tokens_left() } else { i64::MAX }),
        ttl_seconds: b.ttl_seconds.min(settings.child_ttl_seconds),
        actions,
        model_route: route,
        handoff_refs: req.context_handoff_refs.clone(),
    };
    let seq = events::current_seq(conn, run_id)?;
    conn.execute(
        "INSERT INTO spawn_requests(run_id, request_id, parent_agent_id, room_id, spawn_type, payload_json, state, reason_code, effective_json, created_seq, updated_seq)
         VALUES (?1,?2,?3,?4,?5,?6,'REQUESTED','OK',?7,?8,?8)",
        params![run_id, req.request_id, parent.id, room_id, req.spawn_type.as_str(), serde_json::to_string(req)?, serde_json::to_string(&eff)?, seq],
    )?;
    transition(conn, run_id, &req.request_id, room_id, None, SpawnState::Requested, ReasonCode::Ok, "validated")?;
    let needs_approval = !by_operator && settings.spawn_policy == SpawnPolicy::ApprovalRequired;
    if needs_approval {
        transition(conn, run_id, &req.request_id, room_id, Some(SpawnState::Requested), SpawnState::AwaitApproval, ReasonCode::PendingApproval, "operator approval required")?;
        notify(conn, run_id, room_id, &format!("Spawn request from {}: “{}” — awaiting operator approval.", parent.display_name, name))?;
        return Ok(SpawnDecision { request_id: req.request_id.clone(), state: SpawnState::AwaitApproval, reason_code: ReasonCode::PendingApproval, child_agent_id: None, detail: "awaiting approval".into() });
    }
    try_activate(conn, run_id, &req.request_id, SpawnState::Requested)
}

fn notify(conn: &Connection, run_id: &str, room_id: &str, text: &str) -> Result<()> {
    // Spawn notices are operator-visible system lines (other participants learn nothing).
    comms::post(conn, run_id, PostSpec {
        room_id: room_id.into(),
        author_id: "spawn_governor".into(),
        text: text.into(),
        visibility: Some(AudienceScope::OperatorOnly),
        system_origin: Some(OriginKind::System),
        chain_depth: 99,
        ..Default::default()
    })?;
    Ok(())
}

pub fn decide(conn: &Connection, run_id: &str, request_id: &str, approve: bool) -> Result<SpawnDecision> {
    let (state, room_id): (String, String) = conn
        .query_row("SELECT state, room_id FROM spawn_requests WHERE run_id=?1 AND request_id=?2", params![run_id, request_id], |r| Ok((r.get(0)?, r.get(1)?)))
        .optional()?
        .ok_or_else(|| CoreError::NotFound(format!("spawn request {request_id}")))?;
    if state != "AWAIT_APPROVAL" {
        return Err(CoreError::Invalid(format!("request is {state}, not awaiting approval")));
    }
    if !approve {
        transition(conn, run_id, request_id, &room_id, Some(SpawnState::AwaitApproval), SpawnState::Rejected, ReasonCode::CapabilityDenied, "denied by operator")?;
        return Ok(SpawnDecision { request_id: request_id.into(), state: SpawnState::Rejected, reason_code: ReasonCode::CapabilityDenied, child_agent_id: None, detail: "denied by operator".into() });
    }
    try_activate(conn, run_id, request_id, SpawnState::AwaitApproval)
}

fn try_activate(conn: &Connection, run_id: &str, request_id: &str, from: SpawnState) -> Result<SpawnDecision> {
    let (room_id, parent_id, payload, eff): (String, String, String, String) = conn.query_row(
        "SELECT room_id, parent_agent_id, payload_json, effective_json FROM spawn_requests WHERE run_id=?1 AND request_id=?2",
        params![run_id, request_id],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
    )?;
    let settings = room::room_settings(conn, run_id, &room_id)?;
    if active_children_in_room(conn, run_id, &room_id)? >= settings.max_active_children {
        if from != SpawnState::Queued {
            transition(conn, run_id, request_id, &room_id, Some(from), SpawnState::Queued, ReasonCode::Queued, "room child capacity in use; queued without allocating a model process")?;
        }
        return Ok(SpawnDecision { request_id: request_id.into(), state: SpawnState::Queued, reason_code: ReasonCode::Queued, child_agent_id: None, detail: "queued for capacity".into() });
    }
    let req: SpawnRequest = serde_json::from_str(&payload)?;
    let eff: Effective = serde_json::from_str(&eff)?;
    transition(conn, run_id, request_id, &room_id, Some(from), SpawnState::Initializing, ReasonCode::Ok, "capacity available")?;
    let parent = auth::require_agent(conn, run_id, &parent_id)?;
    let n: i64 = conn.query_row("SELECT COUNT(*) FROM agents WHERE run_id=?1 AND parent_agent_id=?2", params![run_id, parent_id], |r| r.get(0))?;
    let child_id = format!("{}-{}-{}", parent_id, slug(&req.display_name), n + 1);
    let child_id: String = child_id.chars().take(48).collect();
    let model_backed = eff.model_route != "demo";
    let persona = format!(
        "You are {}, a short-lived task worker created at the request of {}. You have your own identity and context. \
         You received only the task brief and the explicitly handed-off resources; you do not have {}'s private memory.",
        req.display_name, parent.display_name, parent.display_name
    );
    let mut prof = agent::profile_from_seed(&ProfileSeed {
        id: &child_id,
        name: req.display_name.trim(),
        icon: "🧩",
        role: "task_worker",
        persona: &persona,
        directive: &req.objective,
        cognition: if model_backed { Cognition::L1Reactive } else { Cognition::L0Scripted },
        route: &eff.model_route,
        soma: parent.soma_mode,
        demo_policy: if model_backed { None } else { Some("task_worker") },
        actions: eff.actions.clone(),
        motivation: &req.objective,
        style: "concise",
    });
    prof.limits.turn_output_tokens = (eff.max_output_tokens_total / eff.max_turns.max(1)).clamp(64, 400);
    agent::create_agent(conn, run_id, &prof, AgentKind::Agent, Some(&parent_id), Some(req.spawn_type))?;
    room::add_member(conn, run_id, &room_id, &child_id, "task_worker", "PARENT_ONLY")?;
    // Explicit handoff only: resource-level READ grants, never namespace inheritance (T30).
    for r in &eff.handoff_refs {
        knowledge::grant(conn, run_id, None, Some(r), &child_id, "READ", "spawn_governor")?;
    }
    let now = now_ms();
    conn.execute(
        "INSERT INTO spawn_instances(run_id, child_agent_id, request_id, parent_agent_id, room_id, state, max_turns, max_tokens, ttl_seconds, started_wall_ms, expires_wall_ms)
         VALUES (?1,?2,?3,?4,?5,'ACTIVE',?6,?7,?8,?9,?10)",
        params![run_id, child_id, request_id, parent_id, room_id, eff.max_turns, eff.max_output_tokens_total, eff.ttl_seconds, now, now + eff.ttl_seconds * 1000],
    )?;
    conn.execute("UPDATE spawn_requests SET child_agent_id=?3 WHERE run_id=?1 AND request_id=?2", params![run_id, request_id, child_id])?;
    transition(conn, run_id, request_id, &room_id, Some(SpawnState::Initializing), SpawnState::Active, ReasonCode::Ok, &format!("child {child_id} active; identity + scoped context ready"))?;
    // Task brief through an audited parent→child channel.
    let refs = if eff.handoff_refs.is_empty() { String::new() } else { format!("\nHanded-off resources: {}", eff.handoff_refs.join(", ")) };
    let brief = comms::post(conn, run_id, PostSpec {
        room_id: room_id.clone(),
        author_id: parent_id.clone(),
        text: format!("Task brief for @{}: {}{}\nBudget: {} turns, TTL {}s. Report back with REPORT_FINAL.", req.display_name.trim(), req.objective, refs, eff.max_turns, eff.ttl_seconds),
        visibility: Some(AudienceScope::Direct),
        recipients: vec![child_id.clone()],
        is_demo: parent.cognition == Cognition::L0Scripted,
        chain_depth: 0,
        ..Default::default()
    })?;
    scheduler::on_message(conn, run_id, &brief)?;
    Ok(SpawnDecision { request_id: request_id.into(), state: SpawnState::Active, reason_code: ReasonCode::Ok, child_agent_id: Some(child_id), detail: "child active".into() })
}

/// Completing → Archived; releases the slot and promotes the oldest queued request.
pub fn complete(conn: &Connection, run_id: &str, child_id: &str, reason: &str, result_message_id: Option<&str>) -> Result<()> {
    let row: Option<(String, String, String)> = conn
        .query_row(
            "SELECT request_id, room_id, state FROM spawn_instances WHERE run_id=?1 AND child_agent_id=?2",
            params![run_id, child_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    let Some((request_id, room_id, state)) = row else { return Ok(()) };
    if state == "ARCHIVED" {
        return Ok(());
    }
    conn.execute(
        "UPDATE spawn_instances SET state='COMPLETING', end_reason=?3, result_message_id=COALESCE(?4, result_message_id) WHERE run_id=?1 AND child_agent_id=?2",
        params![run_id, child_id, reason, result_message_id],
    )?;
    transition(conn, run_id, &request_id, &room_id, Some(SpawnState::Active), SpawnState::Completing, ReasonCode::Ok, reason)?;
    agent::set_status(conn, run_id, child_id, AgentStatus::Archived, reason)?;
    if request_id.starts_with("summon_") {
        crate::world::dismiss_entity(conn, run_id, &room_id, child_id)?;
    }
    conn.execute("UPDATE spawn_instances SET state='ARCHIVED' WHERE run_id=?1 AND child_agent_id=?2", params![run_id, child_id])?;
    transition(conn, run_id, &request_id, &room_id, Some(SpawnState::Completing), SpawnState::Archived, ReasonCode::Ok, "final result archived; slot released")?;
    let name = agent::display_name(conn, run_id, child_id);
    notify(conn, run_id, &room_id, &format!("{name} archived ({reason}); slot released."))?;
    // Promote queued requests for this room.
    let queued: Vec<String> = {
        let mut st = conn.prepare("SELECT request_id FROM spawn_requests WHERE run_id=?1 AND room_id=?2 AND state='QUEUED' ORDER BY created_seq")?;
        let v = st.query_map(params![run_id, room_id], |r| r.get(0))?.collect::<rusqlite::Result<Vec<String>>>()?;
        v
    };
    if let Some(next) = queued.first() {
        try_activate(conn, run_id, next, SpawnState::Queued)?;
    }
    Ok(())
}

/// Enforce TTL and turn caps (called by the runner on every tick).
pub fn enforce_limits(conn: &Connection, run_id: &str) -> Result<Vec<String>> {
    let now = now_ms();
    let mut st = conn.prepare(
        "SELECT child_agent_id, CASE WHEN expires_wall_ms < ?2 THEN 'TTL_EXPIRED' WHEN turns_used >= max_turns THEN 'TURN_CAP_REACHED' ELSE 'TOKEN_CAP_REACHED' END
         FROM spawn_instances WHERE run_id=?1 AND state='ACTIVE' AND (expires_wall_ms < ?2 OR turns_used >= max_turns OR tokens_used >= max_tokens)",
    )?;
    let ended = st.query_map(params![run_id, now], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?.collect::<rusqlite::Result<Vec<_>>>()?;
    drop(st);
    let mut out = vec![];
    for (c, why) in ended {
        // Don't cut a child off while its final job is running.
        let running: i64 = conn.query_row("SELECT COUNT(*) FROM jobs WHERE run_id=?1 AND agent_id=?2 AND status IN ('RUNNING')", params![run_id, c], |r| r.get(0))?;
        if running > 0 {
            continue;
        }
        complete(conn, run_id, &c, &why, None)?;
        out.push(c);
    }
    Ok(out)
}

pub fn stop_child(conn: &Connection, run_id: &str, child_id: &str) -> Result<()> {
    complete(conn, run_id, child_id, "STOPPED_BY_OPERATOR", None)
}

/// Crash recovery: no half-initialized children survive; their requests go back to QUEUED.
pub fn recover_after_crash(conn: &Connection) -> Result<()> {
    let mut st = conn.prepare("SELECT run_id, request_id FROM spawn_requests WHERE state='INITIALIZING'")?;
    let rows = st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?.collect::<rusqlite::Result<Vec<_>>>()?;
    drop(st);
    for (run, req) in rows {
        conn.execute("DELETE FROM spawn_instances WHERE run_id=?1 AND request_id=?2 AND state='INITIALIZING'", params![run, req])?;
        conn.execute("UPDATE spawn_requests SET state='QUEUED', reason_code='QUEUED' WHERE run_id=?1 AND request_id=?2", params![run, req])?;
    }
    Ok(())
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct SpawnRow {
    pub request_id: String,
    pub parent_agent_id: String,
    pub parent_name: String,
    pub room_id: String,
    pub spawn_type: String,
    pub state: String,
    pub reason_code: String,
    pub display_name: String,
    pub objective: String,
    pub child_agent_id: Option<String>,
    pub effective: Option<serde_json::Value>,
    pub turns_used: Option<i64>,
    pub max_turns: Option<i64>,
    pub expires_wall_ms: Option<i64>,
    pub end_reason: Option<String>,
}

pub fn list(conn: &Connection, run_id: &str) -> Result<Vec<SpawnRow>> {
    let mut st = conn.prepare(
        "SELECT r.request_id, r.parent_agent_id, COALESCE(a.display_name, r.parent_agent_id), r.room_id, r.spawn_type, r.state, r.reason_code, r.payload_json, r.child_agent_id, r.effective_json,
                i.turns_used, i.max_turns, i.expires_wall_ms, i.end_reason
         FROM spawn_requests r LEFT JOIN agents a ON a.run_id=r.run_id AND a.id=r.parent_agent_id
         LEFT JOIN spawn_instances i ON i.run_id=r.run_id AND i.request_id=r.request_id
         WHERE r.run_id=?1 ORDER BY r.created_seq DESC",
    )?;
    let v = st
        .query_map(params![run_id], |r| {
            let payload: String = r.get(7)?;
            let pv: serde_json::Value = serde_json::from_str(&payload).unwrap_or_default();
            let eff: Option<String> = r.get(9)?;
            Ok(SpawnRow {
                request_id: r.get(0)?,
                parent_agent_id: r.get(1)?,
                parent_name: r.get(2)?,
                room_id: r.get(3)?,
                spawn_type: r.get(4)?,
                state: r.get(5)?,
                reason_code: r.get(6)?,
                display_name: pv.get("display_name").and_then(|v| v.as_str()).or(pv.get("summon").and_then(|v| v.as_str())).unwrap_or("").to_string(),
                objective: pv.get("objective").and_then(|v| v.as_str()).or(pv.get("target").and_then(|v| v.as_str())).unwrap_or("").to_string(),
                child_agent_id: r.get(8)?,
                effective: eff.and_then(|e| serde_json::from_str(&e).ok()),
                turns_used: r.get(10)?,
                max_turns: r.get(11)?,
                expires_wall_ms: r.get(12)?,
                end_reason: r.get(13)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(v)
}
