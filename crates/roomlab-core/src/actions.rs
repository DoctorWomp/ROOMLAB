//! Trusted action gate (§2.3, §13.5). Every proposal — human, scripted (L0) or model output —
//! is schema-checked, authority-checked against current state, and applied in one transaction.
//! A rejection rolls back all mutations and is then journaled with a reason code (T60).

use rusqlite::Connection;
use serde::Deserialize;
use serde_json::{json, Map, Value};

use crate::auth::{self, AgentRow};
use crate::comms::{self, PostSpec, Posted};
use crate::contracts::*;
use crate::error::{reject, CoreError, Result};
use crate::events::{self, NewEvent};
use crate::ids::new_id;
use crate::org::{self, TaskCluster};
use crate::room;
use crate::scheduler::{self, EnqueueSpec};
use crate::soma;
use crate::spawn;
use crate::tools;
use crate::world::{self, WorldCtx};
use crate::Engine;

#[derive(Clone, Debug, Default)]
pub struct ActionContext {
    pub room_id: String,
    pub chain_depth: i64,
    pub is_demo: bool,
    pub model_id: Option<String>,
    pub trigger_event_id: Option<String>,
}

pub fn new_request(run_id: &str, agent_id: &str, based_on: i64, action: ActionKind) -> ActionRequest {
    ActionRequest {
        schema_version: SCHEMA_VERSION.into(),
        request_id: new_id("act"),
        agent_id: agent_id.into(),
        run_id: run_id.into(),
        based_on_event_seq: based_on,
        action,
        args: Map::new(),
        message_text: None,
        reaction_lane: None,
        confidence: None,
    }
}

impl Engine {
    /// Validate + apply an action. Rejections are journaled and returned as `accepted=false`.
    pub fn submit_action(&self, run_id: &str, req: &ActionRequest, ctx: &ActionContext) -> Result<ActionResult> {
        match self.tx(|c| apply_in_tx(c, run_id, req, ctx)) {
            Ok(r) => Ok(r),
            Err(CoreError::Rejected { code, message }) => self.tx(|c| record_rejection(c, run_id, req, ctx, code, &message)),
            Err(e) => Err(e),
        }
    }
}

fn after_post(conn: &Connection, run_id: &str, p: &Posted, out: &mut Vec<String>) -> Result<()> {
    out.push(p.event_id.clone());
    if matches!(p.message.author_kind, OriginKind::Human | OriginKind::Agent) {
        scheduler::on_message(conn, run_id, p)?;
    }
    Ok(())
}

fn resolve_member(conn: &Connection, run_id: &str, room_id: &str, who: &str) -> Result<String> {
    let w = who.trim().trim_start_matches('@').to_lowercase();
    for m in room::members(conn, run_id, room_id)? {
        if m.agent_id.to_lowercase() == w || m.display_name.to_lowercase() == w {
            return Ok(m.agent_id);
        }
    }
    reject(ReasonCode::TargetUnknown, format!("no member '{who}' in this room"))
}

fn text_of(req: &ActionRequest) -> Option<String> {
    req.message_text.clone().or_else(|| req.args.get("text").and_then(|t| t.as_str()).map(String::from)).filter(|t| !t.trim().is_empty())
}

pub fn apply_in_tx(conn: &Connection, run_id: &str, req: &ActionRequest, ctx: &ActionContext) -> Result<ActionResult> {
    if req.schema_version != SCHEMA_VERSION || req.run_id != run_id {
        return reject(ReasonCode::InvalidSchema, "schema_version/run_id mismatch");
    }
    let mode: String = conn.query_row("SELECT mode FROM experiment_runs WHERE id=?1", [run_id], |r| r.get(0))?;
    if mode == "PLAYBACK" {
        return reject(ReasonCode::CapabilityDenied, "playback runs are read-only");
    }
    let actor = auth::require_active(conn, run_id, &req.agent_id)?;
    auth::require_member(conn, run_id, &ctx.room_id, &actor.id)?;
    if req.action == ActionKind::Tool {
        // Unsafe tools are refused before any capability lookup (T56).
        let t = req.args.get("tool").and_then(|t| t.as_str()).unwrap_or("").to_uppercase();
        if !tools::ALLOWED_TOOLS.contains(&t.as_str()) {
            return reject(ReasonCode::UnsafeToolDisabled, format!("tool '{t}' is disabled (no shell/code/network/file execution in this beta)"));
        }
    }
    auth::require_capability(conn, run_id, &actor.id, req.action)?;
    let kind = room::room_kind(conn, run_id, &ctx.room_id)?;
    let is_demo = ctx.is_demo || (actor.cognition == Cognition::L0Scripted && actor.kind != AgentKind::Human);

    // SOMA expression policy shapes permitted output for every author (T45).
    let mut req = req.clone();
    let mut soma_adjust = vec![];
    if let Some(t) = text_of(&req) {
        let mode = if room::room_settings(conn, run_id, &ctx.room_id)?.soma_enabled { actor.soma_mode } else { SomaMode::Off };
        let mode = if actor.kind == AgentKind::Human { SomaMode::Expressive } else { mode };
        let o = soma::apply_policy(mode, &t, req.reaction_lane.clone())?;
        req.message_text = Some(o.text);
        req.reaction_lane = o.lane;
        soma_adjust = o.adjustments;
    } else if let Some(l) = &req.reaction_lane {
        soma::validate_lane(l)?;
    }

    let mut ev_ids: Vec<String> = vec![];
    let detail: String;
    let world_actions = [ActionKind::Look, ActionKind::Inspect, ActionKind::Move, ActionKind::Take, ActionKind::Drop, ActionKind::Use, ActionKind::Summon];
    let in_story = kind == RoomKind::Story;

    match req.action {
        a if world_actions.contains(&a) || (in_story && matches!(a, ActionKind::Speak | ActionKind::Ask | ActionKind::Whisper | ActionKind::Wait | ActionKind::NoOp)) => {
            let out = world::apply(conn, run_id, &actor, &req, &WorldCtx { room_id: &ctx.room_id, chain_depth: ctx.chain_depth, trigger_event_id: ctx.trigger_event_id.as_deref() })?;
            for p in &out.posted {
                if matches!(p.message.author_kind, OriginKind::Human | OriginKind::Agent) {
                    scheduler::on_message(conn, run_id, p)?;
                }
            }
            if matches!(a, ActionKind::Wait | ActionKind::NoOp) {
                scheduler::log_turn(conn, run_id, &actor.id, Some(&ctx.room_id), ctx.trigger_event_id.as_deref(), TurnOutcome::ExplicitNoop, "chose to wait")?;
            }
            ev_ids.extend(out.event_ids);
            detail = out.summary;
        }
        ActionKind::Speak | ActionKind::Ask => {
            let text = text_of(&req).ok_or_else(|| CoreError::Rejected { code: ReasonCode::InvalidSchema, message: "SPEAK requires message_text".into() })?;
            let p = comms::post(conn, run_id, PostSpec {
                room_id: ctx.room_id.clone(),
                author_id: actor.id.clone(),
                text,
                visibility: Some(AudienceScope::RoomMembers),
                reaction_lane: req.reaction_lane.clone(),
                confidence: req.confidence,
                is_demo,
                model_id: ctx.model_id.clone(),
                chain_depth: ctx.chain_depth,
                parents: ctx.trigger_event_id.iter().cloned().collect(),
                ..Default::default()
            })?;
            after_post(conn, run_id, &p, &mut ev_ids)?;
            detail = format!("message {}", p.message.message_id);
        }
        ActionKind::Whisper => {
            let to = req.args.get("to").and_then(|t| t.as_str()).ok_or_else(|| CoreError::Rejected { code: ReasonCode::InvalidSchema, message: "WHISPER requires args.to".into() })?;
            let to = resolve_member(conn, run_id, &ctx.room_id, to)?;
            let text = text_of(&req).ok_or_else(|| CoreError::Rejected { code: ReasonCode::InvalidSchema, message: "WHISPER requires message_text".into() })?;
            let p = comms::post(conn, run_id, PostSpec {
                room_id: ctx.room_id.clone(),
                author_id: actor.id.clone(),
                text,
                visibility: Some(AudienceScope::Direct),
                recipients: vec![to],
                reaction_lane: req.reaction_lane.clone(),
                confidence: req.confidence,
                is_demo,
                model_id: ctx.model_id.clone(),
                chain_depth: ctx.chain_depth,
                parents: ctx.trigger_event_id.iter().cloned().collect(),
                ..Default::default()
            })?;
            after_post(conn, run_id, &p, &mut ev_ids)?;
            detail = format!("direct message {}", p.message.message_id);
        }
        ActionKind::NoOp | ActionKind::Wait => {
            scheduler::log_turn(conn, run_id, &actor.id, Some(&ctx.room_id), ctx.trigger_event_id.as_deref(), TurnOutcome::ExplicitNoop, "chose silence")?;
            detail = "explicit NO_OP".into();
        }
        ActionKind::RequestSpawn => {
            let sr = spawn_request_from_args(conn, run_id, &actor, &ctx.room_id, &req.args)?;
            let d = spawn::request(conn, run_id, &ctx.room_id, &sr, actor.kind == AgentKind::Human)?;
            detail = format!("spawn {} → {}", d.request_id, d.state.as_str());
        }
        ActionKind::Tool => {
            let out = tools::invoke(conn, run_id, &actor.id, &req.args)?;
            let p = comms::post(conn, run_id, PostSpec {
                room_id: ctx.room_id.clone(),
                author_id: "tool_broker".into(),
                text: format!("{} → {}: {}", actor.display_name, out.tool, out.summary),
                visibility: Some(AudienceScope::Location),
                audience_limit: Some(vec![actor.id.clone()]),
                system_origin: Some(OriginKind::Tool),
                chain_depth: 99,
                ..Default::default()
            })?;
            ev_ids.push(p.event_id.clone());
            let sim = events::sim_time(conn, run_id)?;
            comms::record_observation(conn, run_id, &actor.id, Some(&ctx.room_id), &p.event_id, ObsChannel::System, ObsKind::ActionResult, json!({"tool": out.tool, "result": out.result}), ObsStatus::Observed, "tool_broker_v1", sim, None, p.seq)?;
            if actor.kind != AgentKind::Human {
                // Give the agent a turn to use the result (bounded by chain depth and budgets).
                scheduler::enqueue(conn, run_id, EnqueueSpec {
                    agent_id: &actor.id,
                    room_id: &ctx.room_id,
                    trigger_event_id: &p.event_id,
                    trigger_message_id: None,
                    priority: scheduler::PRIO_TASK,
                    chain_depth: ctx.chain_depth + 1,
                    reason: "tool result",
                })?;
            }
            detail = out.summary;
        }
        ActionKind::ReportProgress | ActionKind::ReportFinal | ActionKind::AskParent => {
            let parent = actor.parent_agent_id.clone().ok_or_else(|| CoreError::Rejected { code: ReasonCode::CapabilityDenied, message: "only spawned children report to a parent".into() })?;
            let text = text_of(&req).ok_or_else(|| CoreError::Rejected { code: ReasonCode::InvalidSchema, message: "report requires message_text".into() })?;
            let label = match req.action {
                ActionKind::ReportFinal => "FINAL REPORT",
                ActionKind::ReportProgress => "PROGRESS",
                _ => "QUESTION",
            };
            let p = comms::post(conn, run_id, PostSpec {
                room_id: ctx.room_id.clone(),
                author_id: actor.id.clone(),
                text: format!("[{label}] {text}"),
                visibility: Some(AudienceScope::ParentOnly),
                recipients: vec![parent.clone()],
                reaction_lane: req.reaction_lane.clone(),
                confidence: req.confidence,
                is_demo,
                model_id: ctx.model_id.clone(),
                chain_depth: ctx.chain_depth,
                parents: ctx.trigger_event_id.iter().cloned().collect(),
                ..Default::default()
            })?;
            after_post(conn, run_id, &p, &mut ev_ids)?;
            if req.action == ActionKind::ReportFinal {
                spawn::complete(conn, run_id, &actor.id, "REPORT_FINAL", Some(&p.message.message_id))?;
            }
            detail = format!("{label} to {parent}");
        }
        ActionKind::AssignTask => {
            let is_human = actor.kind == AgentKind::Human;
            // Delegation is an org relation; it grants nothing beyond task routing (T35).
            if !is_human && !org::has_edge(conn, run_id, &ctx.room_id, &actor.id, OrgRelation::CanDelegate)? {
                return reject(ReasonCode::CapabilityDenied, format!("{} has no can_delegate edge in this room", actor.display_name));
            }
            let goal = req.args.get("goal").and_then(|g| g.as_str()).ok_or_else(|| CoreError::Rejected { code: ReasonCode::InvalidSchema, message: "ASSIGN_TASK requires args.goal".into() })?;
            let assignees_raw: Vec<String> = req.args.get("assignees").and_then(|a| serde_json::from_value(a.clone()).ok()).unwrap_or_default();
            if assignees_raw.is_empty() || assignees_raw.len() > 6 {
                return reject(ReasonCode::InvalidSchema, "ASSIGN_TASK requires 1–6 assignees");
            }
            let mut assignees = vec![];
            for a in &assignees_raw {
                assignees.push(resolve_member(conn, run_id, &ctx.room_id, a)?);
            }
            let criteria: Vec<String> = req.args.get("verification_criteria").and_then(|a| serde_json::from_value(a.clone()).ok()).unwrap_or_else(|| {
                vec!["Both competing explanations recorded".into(), "Supporting and contradicting evidence referenced".into(), "No unauthorized disclosure".into()]
            });
            let cid = org::next_cluster_id(conn, run_id)?;
            org::create_task(conn, run_id, &TaskCluster {
                cluster_id: cid.clone(),
                room_id: ctx.room_id.clone(),
                goal: goal.chars().take(1000).collect(),
                owner_agent_id: actor.id.clone(),
                status: TaskStatus::InProgress,
                assignees: assignees.clone(),
                priority: 2,
                dependencies: vec![],
                verification_criteria: criteria,
                human_action_needed: false,
                notes: vec![],
            }, &actor.id)?;
            let names: Vec<String> = assignees.iter().map(|a| crate::agent::display_name(conn, run_id, a)).collect();
            let p = comms::post(conn, run_id, PostSpec {
                room_id: ctx.room_id.clone(),
                author_id: actor.id.clone(),
                text: req.message_text.clone().unwrap_or_else(|| format!("📋 {cid}: {} — assigned to {}.", goal.chars().take(160).collect::<String>(), names.join(", "))),
                visibility: Some(AudienceScope::RoomMembers),
                is_demo,
                model_id: ctx.model_id.clone(),
                chain_depth: 99, // the announcement itself doesn't trigger; the briefs below do
                ..Default::default()
            })?;
            ev_ids.push(p.event_id.clone());
            for (a, n) in assignees.iter().zip(&names) {
                let brief = comms::post(conn, run_id, PostSpec {
                    room_id: ctx.room_id.clone(),
                    author_id: actor.id.clone(),
                    text: format!("{cid} brief for @{n}: {goal}"),
                    visibility: Some(AudienceScope::Direct),
                    recipients: vec![a.clone()],
                    is_demo,
                    model_id: ctx.model_id.clone(),
                    chain_depth: ctx.chain_depth,
                    ..Default::default()
                })?;
                after_post(conn, run_id, &brief, &mut ev_ids)?;
            }
            detail = format!("created {cid}");
        }
        ActionKind::UpdateTask => {
            let cid = req.args.get("cluster_id").and_then(|c| c.as_str()).ok_or_else(|| CoreError::Rejected { code: ReasonCode::InvalidSchema, message: "UPDATE_TASK requires args.cluster_id".into() })?;
            let status: Option<TaskStatus> = match req.args.get("status").and_then(|s| s.as_str()) {
                Some(s) => Some(parse_enum(s).map_err(|_| CoreError::Rejected { code: ReasonCode::InvalidSchema, message: format!("bad status {s}") })?),
                None => None,
            };
            let note = req.args.get("note").and_then(|n| n.as_str());
            org::update_task(conn, run_id, cid, &actor.id, actor.kind == AgentKind::Human, status, note)?;
            if let Some(t) = text_of(&req) {
                let p = comms::post(conn, run_id, PostSpec {
                    room_id: ctx.room_id.clone(),
                    author_id: actor.id.clone(),
                    text: t,
                    visibility: Some(AudienceScope::RoomMembers),
                    is_demo,
                    model_id: ctx.model_id.clone(),
                    chain_depth: 99,
                    ..Default::default()
                })?;
                ev_ids.push(p.event_id.clone());
            }
            detail = format!("updated {cid}");
        }
        other => return reject(ReasonCode::CapabilityDenied, format!("{} is not valid in a {} room", other.as_str(), kind.as_str())),
    }

    let origin = if actor.kind == AgentKind::Human { OriginKind::Human } else { OriginKind::Agent };
    let ev = events::append(
        conn,
        run_id,
        NewEvent::new(EventType::ActionExecuted, origin, &actor.id, AudienceScope::OperatorOnly, json!({
            "request": req, "accepted": true, "detail": detail, "soma_adjustments": soma_adjust, "is_demo": is_demo, "model_id": ctx.model_id
        }))
        .room(&ctx.room_id)
        .parents(ctx.trigger_event_id.iter().cloned().collect()),
    )?;
    ev_ids.push(ev.event_id.clone());
    Ok(ActionResult {
        request_id: req.request_id.clone(),
        accepted: true,
        executed: true,
        reason_code: ReasonCode::Ok,
        result_event_ids: ev_ids,
        resolved_at_seq: ev.seq,
        detail: Some(detail),
    })
}

pub fn record_rejection(conn: &Connection, run_id: &str, req: &ActionRequest, ctx: &ActionContext, code: ReasonCode, message: &str) -> Result<ActionResult> {
    let actor_known = auth::load_agent(conn, run_id, &req.agent_id)?;
    let origin = match actor_known.as_ref().map(|a| a.kind) {
        Some(AgentKind::Human) => OriginKind::Human,
        Some(_) => OriginKind::Agent,
        None => OriginKind::System,
    };
    let room_exists = room::room_kind(conn, run_id, &ctx.room_id).is_ok();
    let mut ne = NewEvent::new(EventType::ActionRejected, origin, &req.agent_id, AudienceScope::OperatorOnly, json!({
        "request": req, "reason_code": code, "reason": message
    }))
    .parents(ctx.trigger_event_id.iter().cloned().collect());
    if room_exists {
        ne = ne.room(&ctx.room_id);
    }
    let ev = events::append(conn, run_id, ne)?;
    if room_exists {
        if let Some(a) = &actor_known {
            // The actor learns *why* (local explanatory reason); others learn nothing.
            let sim = events::sim_time(conn, run_id)?;
            comms::record_observation(conn, run_id, &a.id, Some(&ctx.room_id), &ev.event_id, ObsChannel::System, ObsKind::ActionResult,
                json!({"rejected": req.action, "reason_code": code, "reason": message}), ObsStatus::Observed, "action_gate_v1", sim, None, ev.seq)?;
            let audience = if a.kind == AgentKind::Human { vec![a.id.clone()] } else { vec![] };
            comms::post(conn, run_id, PostSpec {
                room_id: ctx.room_id.clone(),
                author_id: "action_gate".into(),
                text: format!("⛔ {} {} rejected — {}: {}", a.display_name, req.action.as_str(), code.as_str(), message),
                visibility: Some(if audience.is_empty() { AudienceScope::OperatorOnly } else { AudienceScope::Location }),
                audience_limit: Some(audience),
                system_origin: Some(OriginKind::System),
                chain_depth: 99,
                ..Default::default()
            })?;
        }
    }
    Ok(ActionResult {
        request_id: req.request_id.clone(),
        accepted: false,
        executed: false,
        reason_code: code,
        result_event_ids: vec![ev.event_id],
        resolved_at_seq: ev.seq,
        detail: Some(message.to_string()),
    })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SpawnArgs {
    display_name: String,
    objective: String,
    #[serde(default)]
    context_handoff_refs: Vec<String>,
    #[serde(default)]
    requested_actions: Option<Vec<ActionKind>>,
    #[serde(default)]
    requested_budget: Option<SpawnBudget>,
    #[serde(default)]
    requested_model_route: Option<String>,
    #[serde(default)]
    spawn_type: Option<SpawnType>,
}

fn spawn_request_from_args(conn: &Connection, run_id: &str, actor: &AgentRow, room_id: &str, args: &Map<String, Value>) -> Result<SpawnRequest> {
    let a: SpawnArgs = serde_json::from_value(Value::Object(args.clone()))
        .map_err(|e| CoreError::Rejected { code: ReasonCode::InvalidSchema, message: format!("REQUEST_SPAWN args: {e}") })?;
    let s = room::room_settings(conn, run_id, room_id)?;
    Ok(SpawnRequest {
        schema_version: SCHEMA_VERSION.into(),
        request_id: new_id("spawn_req"),
        run_id: run_id.into(),
        parent_agent_id: actor.id.clone(),
        spawn_type: a.spawn_type.unwrap_or(SpawnType::TaskWorker),
        display_name: a.display_name,
        requested_model_route: a.requested_model_route.unwrap_or_else(|| "room_default_subagent".into()),
        objective: a.objective,
        requested_visibility: SpawnVisibility::ParentOnly,
        context_handoff_refs: a.context_handoff_refs,
        requested_actions: a.requested_actions.unwrap_or_else(|| vec![ActionKind::Speak, ActionKind::Tool, ActionKind::ReportFinal, ActionKind::NoOp]),
        requested_budget: a.requested_budget.unwrap_or(SpawnBudget { max_turns: s.child_max_turns, max_output_tokens_total: s.child_max_output_tokens, ttl_seconds: s.child_ttl_seconds }),
    })
}

// ───────────────────────────── Model output parsing (T23) ─────────────────────────────

#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct ModelAction {
    pub action: ActionKind,
    #[serde(default)]
    pub message_text: Option<String>,
    #[serde(default)]
    pub args: Map<String, Value>,
    #[serde(default)]
    pub reaction_lane: Option<ReactionLane>,
    #[serde(default)]
    pub confidence: Option<Confidence>,
}

#[derive(Debug, Clone)]
pub enum ParseOutcome {
    Action(ModelAction),
    /// Plain prose accepted as SPEAK (lowest-privilege action).
    PlainSpeech(String),
    Invalid(String),
    Empty,
}

/// Remove reasoning blocks the provider may emit inline.
pub fn strip_reasoning(raw: &str) -> String {
    let mut s = raw.to_string();
    for (open, close) in [("<think>", "</think>"), ("<thinking>", "</thinking>")] {
        while let Some(i) = s.find(open) {
            match s[i..].find(close) {
                Some(j) => s.replace_range(i..i + j + close.len(), ""),
                None => s.truncate(i),
            }
        }
    }
    s.trim().to_string()
}

fn extract_json(s: &str) -> Option<&str> {
    let start = s.find('{')?;
    let mut depth = 0i32;
    let mut in_str = false;
    let mut esc = false;
    for (i, c) in s[start..].char_indices() {
        if in_str {
            match (esc, c) {
                (true, _) => esc = false,
                (false, '\\') => esc = true,
                (false, '"') => in_str = false,
                _ => {}
            }
            continue;
        }
        match c {
            '"' => in_str = true,
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&s[start..start + i + 1]);
                }
            }
            _ => {}
        }
    }
    None
}

/// Normalise harmless representational variants (case of enum names, lane as list/string).
fn normalise(v: &mut Value) {
    if let Some(o) = v.as_object_mut() {
        if let Some(Value::String(a)) = o.get_mut("action") {
            *a = a.trim().to_uppercase().replace([' ', '-'], "_");
        }
        if let Some(Value::String(c)) = o.get_mut("confidence") {
            *c = c.trim().to_uppercase().replace([' ', '-'], "_");
        }
        let lane_fix = match o.get("reaction_lane") {
            Some(Value::Array(a)) => Some(json!({"owner": "SOMA_EXPRESSION", "steps": a})),
            Some(Value::String(s)) => Some(json!({"owner": "SOMA_EXPRESSION", "steps": soma::parse_lane_text(s)})),
            Some(Value::Null) => None,
            _ => None,
        };
        if let Some(l) = lane_fix {
            o.insert("reaction_lane".into(), l);
        }
        if matches!(o.get("reaction_lane"), Some(Value::Null)) {
            o.remove("reaction_lane");
        }
        if matches!(o.get("args"), Some(Value::Null)) {
            o.remove("args");
        }
    }
}

pub fn parse_model_output(raw: &str, plain_text_as_speak: bool) -> ParseOutcome {
    let s = strip_reasoning(raw);
    let s = s.trim_start_matches("```json").trim_start_matches("```").trim_end_matches("```").trim();
    if s.is_empty() {
        return ParseOutcome::Empty;
    }
    match extract_json(s) {
        Some(j) => {
            let mut v: Value = match serde_json::from_str(j) {
                Ok(v) => v,
                Err(e) => return ParseOutcome::Invalid(format!("malformed JSON: {e}")),
            };
            normalise(&mut v);
            match serde_json::from_value::<ModelAction>(v) {
                Ok(a) => ParseOutcome::Action(a),
                Err(e) => ParseOutcome::Invalid(format!("schema: {e}")),
            }
        }
        None if s.contains('{') => ParseOutcome::Invalid("unterminated JSON object".into()),
        None if plain_text_as_speak => ParseOutcome::PlainSpeech(s.chars().take(comms::MAX_TEXT_CHARS).collect()),
        None => ParseOutcome::Invalid("no JSON action object".into()),
    }
}

pub fn to_request(run_id: &str, agent_id: &str, based_on: i64, a: ModelAction) -> ActionRequest {
    ActionRequest {
        schema_version: SCHEMA_VERSION.into(),
        request_id: new_id("act"),
        agent_id: agent_id.into(),
        run_id: run_id.into(),
        based_on_event_seq: based_on,
        action: a.action,
        args: a.args,
        message_text: a.message_text,
        reaction_lane: a.reaction_lane,
        confidence: a.confidence,
    }
}
