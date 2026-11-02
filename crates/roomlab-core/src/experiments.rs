//! Runs, manifests, interventions, export/import, playback and fork (§13.1, §15).

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::contracts::*;
use crate::error::{CoreError, Result};
use crate::events::{self, EventFilter, NewEvent};
use crate::ids::{new_id, now_utc};
use crate::redact;
use crate::{Engine, ViewRole, HUMAN_ID, WORKSPACE_ID};

pub struct NewRun<'a> {
    pub name: &'a str,
    pub scenario_id: Option<&'a str>,
    pub session_type: SessionType,
    pub mode: RunMode,
    pub seed: i64,
    pub budgets: RunBudgets,
    pub spawn_policy: SpawnPolicy,
    pub conditions: Vec<ConditionSpec>,
    pub parent_run_id: Option<&'a str>,
}

impl Default for RunBudgets {
    fn default() -> Self {
        RunBudgets { output_tokens: 60_000, agent_turns: 400, max_chain_depth: 6 }
    }
}

pub fn route_snapshot(conn: &Connection, provider: &ProviderConfig) -> Result<Vec<RouteSnapshot>> {
    let mut st = conn.prepare("SELECT id, provider_kind, model_id FROM model_routes WHERE enabled=1 ORDER BY id")?;
    let rows = st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, Option<String>>(2)?)))?.collect::<rusqlite::Result<Vec<_>>>()?;
    let host = url::Url::parse(&provider.base_url).ok().and_then(|u| u.host_str().map(|h| format!("{h}:{}", u.port_or_known_default().unwrap_or(0))));
    rows.into_iter()
        .map(|(id, kind, model)| {
            let k: ProviderKind = parse_enum(&kind)?;
            let is_demo = k == ProviderKind::Demo;
            Ok(RouteSnapshot {
                route_id: id,
                provider_kind: if is_demo { k } else { provider.provider_kind },
                model_id: if is_demo { None } else { model.or_else(|| provider.model_id.clone()) },
                endpoint_host: if is_demo { None } else { host.clone() },
            })
        })
        .collect()
}

pub fn create_run(conn: &Connection, r: NewRun, provider: &ProviderConfig) -> Result<String> {
    let id = new_id("run");
    let manifest = RunManifest {
        schema_version: SCHEMA_VERSION.into(),
        app_version: APP_VERSION.into(),
        run_id: id.clone(),
        name: r.name.into(),
        scenario_id: r.scenario_id.map(String::from),
        session_type: r.session_type,
        mode: r.mode,
        seed: r.seed,
        created_at: now_utc(),
        parent_run_id: r.parent_run_id.map(String::from),
        conditions: r.conditions.clone(),
        model_routes: route_snapshot(conn, provider)?,
        budgets: r.budgets.clone(),
        spawn_policy: r.spawn_policy,
    };
    conn.execute(
        "INSERT INTO experiment_runs(id, workspace_id, name, scenario_id, session_type, mode, seed, manifest_json, budget_output_tokens, budget_turns, max_chain_depth, spawn_policy, parent_run_id, created_at)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14)",
        params![
            id, WORKSPACE_ID, r.name, r.scenario_id, r.session_type.as_str(), r.mode.as_str(), r.seed, serde_json::to_string(&manifest)?,
            r.budgets.output_tokens, r.budgets.agent_turns, r.budgets.max_chain_depth, r.spawn_policy.as_str(), r.parent_run_id, manifest.created_at
        ],
    )?;
    for c in &r.conditions {
        conn.execute(
            "INSERT INTO experiment_conditions(run_id, id, arm, key, value_json) VALUES (?1,?2,?3,?4,?5)",
            params![id, new_id("cond"), c.arm, c.key, c.value.to_string()],
        )?;
    }
    events::append(conn, &id, NewEvent::system(EventType::RunCreated, serde_json::to_value(&manifest)?))?;
    crate::agent::ensure_human(conn, &id)?;
    Ok(id)
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct RunSummary {
    pub id: String,
    pub name: String,
    pub scenario_id: Option<String>,
    pub session_type: String,
    pub mode: String,
    pub contaminated: bool,
    pub created_at: String,
    pub events: i64,
    pub parent_run_id: Option<String>,
}

pub fn list_runs(conn: &Connection) -> Result<Vec<RunSummary>> {
    let mut st = conn.prepare("SELECT id, name, scenario_id, session_type, mode, contaminated, created_at, next_seq-1, parent_run_id FROM experiment_runs ORDER BY created_at DESC")?;
    let v = st
        .query_map([], |r| {
            Ok(RunSummary {
                id: r.get(0)?,
                name: r.get(1)?,
                scenario_id: r.get(2)?,
                session_type: r.get(3)?,
                mode: r.get(4)?,
                contaminated: r.get::<_, i64>(5)? != 0,
                created_at: r.get(6)?,
                events: r.get(7)?,
                parent_run_id: r.get(8)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(v)
}

pub fn manifest(conn: &Connection, run_id: &str) -> Result<RunManifest> {
    let j: String = conn
        .query_row("SELECT manifest_json FROM experiment_runs WHERE id=?1", params![run_id], |r| r.get(0))
        .optional()?
        .ok_or_else(|| CoreError::NotFound(format!("run {run_id}")))?;
    Ok(serde_json::from_str(&j)?)
}

pub fn session_type(conn: &Connection, run_id: &str) -> Result<SessionType> {
    let s: String = conn.query_row("SELECT session_type FROM experiment_runs WHERE id=?1", params![run_id], |r| r.get(0))?;
    parse_enum(&s)
}

/// Journal an intervention; controlled trials become contaminated unless preauthorized (T55).
pub fn record_intervention(conn: &Connection, run_id: &str, actor: &str, description: &str, before: &Value, after: &Value, preauthorized: bool) -> Result<bool> {
    let ev = events::append(
        conn,
        run_id,
        NewEvent::new(EventType::Intervention, OriginKind::Human, actor, AudienceScope::OperatorOnly, json!({
            "description": description, "before": before, "after": after, "preauthorized": preauthorized
        })),
    )?;
    conn.execute(
        "INSERT INTO interventions(run_id, id, actor, description, before_json, after_json, seq, preauthorized) VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
        params![run_id, new_id("int"), actor, description, before.to_string(), after.to_string(), ev.seq, preauthorized as i64],
    )?;
    let contaminate = session_type(conn, run_id)? == SessionType::ControlledTrial && !preauthorized;
    if contaminate {
        conn.execute("UPDATE experiment_runs SET contaminated=1 WHERE id=?1", params![run_id])?;
    }
    Ok(contaminate)
}

#[derive(Deserialize, Serialize, Clone, Debug)]
#[serde(tag = "type", rename_all = "SCREAMING_SNAKE_CASE", deny_unknown_fields)]
pub enum DirectorChange {
    MoveEntity { room_id: String, entity_id: String, to_location: String },
    SetEntityState { room_id: String, entity_id: String, state: String },
    SetBudget { output_tokens: Option<i64>, agent_turns: Option<i64> },
}

/// Director edits are journaled with before/after state; never a silent retcon (§6.5).
pub fn director_edit(conn: &Connection, run_id: &str, change: &DirectorChange, description: &str, preauthorized: bool) -> Result<bool> {
    let (before, after) = match change {
        DirectorChange::MoveEntity { room_id, entity_id, to_location } => {
            let w = crate::world::world_for_room(conn, run_id, room_id)?;
            let e = crate::world::entity(conn, run_id, &w.world_id, entity_id)?.ok_or_else(|| CoreError::NotFound(entity_id.clone()))?;
            let seq = events::current_seq(conn, run_id)?;
            conn.execute("DELETE FROM inventories WHERE run_id=?1 AND world_id=?2 AND entity_id=?3", params![run_id, w.world_id, entity_id])?;
            conn.execute("UPDATE world_entities SET location_id=?4, rev=?5 WHERE run_id=?1 AND world_id=?2 AND id=?3", params![run_id, w.world_id, entity_id, to_location, seq])?;
            (json!({"entity": entity_id, "location": e.location}), json!({"entity": entity_id, "location": to_location}))
        }
        DirectorChange::SetEntityState { room_id, entity_id, state } => {
            let w = crate::world::world_for_room(conn, run_id, room_id)?;
            let e = crate::world::entity(conn, run_id, &w.world_id, entity_id)?.ok_or_else(|| CoreError::NotFound(entity_id.clone()))?;
            let seq = events::current_seq(conn, run_id)?;
            conn.execute("UPDATE world_entities SET state=?4, rev=?5 WHERE run_id=?1 AND world_id=?2 AND id=?3", params![run_id, w.world_id, entity_id, state, seq])?;
            (json!({"entity": entity_id, "state": e.state}), json!({"entity": entity_id, "state": state}))
        }
        DirectorChange::SetBudget { output_tokens, agent_turns } => {
            let (bt, bu): (i64, i64) = conn.query_row("SELECT budget_output_tokens, budget_turns FROM experiment_runs WHERE id=?1", params![run_id], |r| Ok((r.get(0)?, r.get(1)?)))?;
            let nt = output_tokens.unwrap_or(bt).max(0);
            let nu = agent_turns.unwrap_or(bu).max(0);
            conn.execute("UPDATE experiment_runs SET budget_output_tokens=?2, budget_turns=?3 WHERE id=?1", params![run_id, nt, nu])?;
            (json!({"output_tokens": bt, "agent_turns": bu}), json!({"output_tokens": nt, "agent_turns": nu}))
        }
    };
    record_intervention(conn, run_id, HUMAN_ID, description, &before, &after, preauthorized)
}

impl Engine {
    /// Backend-enforced view role switch (§12.6). Leaving participant role during a
    /// controlled trial requires explicit acknowledgement and contaminates the run.
    pub fn set_view_role(&self, run_id: Option<&str>, role: ViewRole, acknowledge_contamination: bool) -> Result<bool> {
        let current = self.view_role();
        if current == role {
            return Ok(false);
        }
        let mut contaminated = false;
        if let Some(run_id) = run_id {
            contaminated = self.tx(|c| {
                let controlled = session_type(c, run_id)? == SessionType::ControlledTrial;
                if role == ViewRole::Operator && controlled && !acknowledge_contamination {
                    return Err(CoreError::Forbidden("Opening operator truth during a controlled trial contaminates the run. Acknowledge to continue.".into()));
                }
                events::append(c, run_id, NewEvent::new(EventType::ViewRoleChanged, OriginKind::Human, HUMAN_ID, AudienceScope::OperatorOnly, json!({"from": current, "to": role})))?;
                if role == ViewRole::Operator && controlled {
                    return record_intervention(c, run_id, HUMAN_ID, "participant opened operator truth view", &json!({"view": current}), &json!({"view": role}), false);
                }
                Ok(false)
            })?;
        }
        self.set_view_role_internal(role);
        Ok(contaminated)
    }
}

// ───────────────────────────── Export / import / playback / fork ─────────────────────────────

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ExportKind {
    /// Participant-safe: public messages, non-operator events, public/room knowledge.
    Public,
    /// Operator: everything except credentials; sealed keys only if revealed.
    Operator,
}

fn query_json(conn: &Connection, sql: &str, run_id: &str) -> Result<Vec<Value>> {
    let mut st = conn.prepare(sql)?;
    let cols: Vec<String> = st.column_names().iter().map(|s| s.to_string()).collect();
    let rows = st
        .query_map(params![run_id], |r| {
            let mut m = serde_json::Map::new();
            for (i, c) in cols.iter().enumerate() {
                let v: rusqlite::types::Value = r.get(i)?;
                let j = match v {
                    rusqlite::types::Value::Null => Value::Null,
                    rusqlite::types::Value::Integer(n) => json!(n),
                    rusqlite::types::Value::Real(f) => json!(f),
                    rusqlite::types::Value::Text(t) => json!(t),
                    rusqlite::types::Value::Blob(_) => Value::Null,
                };
                m.insert(c.clone(), j);
            }
            Ok(Value::Object(m))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

pub const EXPORT_FORMAT: &str = "roomlab-run-export-v1";

pub fn export_run(conn: &Connection, run_id: &str, kind: ExportKind, secrets: &[String]) -> Result<String> {
    let m = manifest(conn, run_id)?;
    let op = kind == ExportKind::Operator;
    let messages = if op {
        query_json(conn, "SELECT * FROM messages WHERE run_id=?1 ORDER BY seq", run_id)?
    } else {
        query_json(conn, "SELECT * FROM messages WHERE run_id=?1 AND visibility='ROOM_MEMBERS' AND author_kind IN ('HUMAN','AGENT','NARRATOR') ORDER BY seq", run_id)?
    };
    let events = if op {
        query_json(conn, "SELECT * FROM events WHERE run_id=?1 ORDER BY seq", run_id)?
    } else {
        query_json(conn, "SELECT * FROM events WHERE run_id=?1 AND audience_scope IN ('ROOM_MEMBERS','PUBLIC') ORDER BY seq", run_id)?
    };
    let knowledge = if op {
        query_json(conn, "SELECT r.* FROM knowledge_resources r WHERE r.run_id=?1 AND NOT (r.kind='HIDDEN_TEST_KEY' AND r.visibility='SEALED')", run_id)?
    } else {
        query_json(conn, "SELECT * FROM knowledge_resources WHERE run_id=?1 AND visibility='PUBLIC'", run_id)?
    };
    let agents = query_json(conn, "SELECT id, display_name, icon, kind, cognition, model_route, status, parent_agent_id, spawn_type, soma_mode, demo_policy, created_at, updated_at FROM agents WHERE run_id=?1", run_id)?;
    let mut bundle = json!({
        "format": EXPORT_FORMAT,
        "export_kind": kind,
        "exported_at": now_utc(),
        "manifest": m,
        "rooms": query_json(conn, "SELECT * FROM rooms WHERE run_id=?1", run_id)?,
        "channels": query_json(conn, "SELECT * FROM channels WHERE run_id=?1", run_id)?,
        "agents": agents,
        "memberships": query_json(conn, "SELECT * FROM memberships WHERE run_id=?1", run_id)?,
        "messages": messages,
        "events": events,
        "knowledge": knowledge,
        "tasks": query_json(conn, "SELECT * FROM tasks WHERE run_id=?1", run_id)?,
        "org_edges": query_json(conn, "SELECT * FROM org_edges WHERE run_id=?1", run_id)?,
        "notice": "Export contains no provider credentials. Public exports exclude private messages, operator-only events, private knowledge and sealed answer keys."
    });
    if op {
        bundle["agent_profiles"] = json!(query_json(conn, "SELECT * FROM agent_profiles WHERE run_id=?1", run_id)?);
        bundle["observations"] = json!(query_json(conn, "SELECT * FROM observations WHERE run_id=?1 ORDER BY seq", run_id)?);
        bundle["spawn_requests"] = json!(query_json(conn, "SELECT * FROM spawn_requests WHERE run_id=?1", run_id)?);
        bundle["provider_requests"] = json!(query_json(conn, "SELECT * FROM provider_requests WHERE run_id=?1", run_id)?);
        bundle["interventions"] = json!(query_json(conn, "SELECT * FROM interventions WHERE run_id=?1", run_id)?);
        bundle["world_entities"] = json!(query_json(conn, "SELECT * FROM world_entities WHERE run_id=?1", run_id)?);
        bundle["turn_log"] = json!(query_json(conn, "SELECT * FROM turn_log WHERE run_id=?1 ORDER BY seq", run_id)?);
    }
    let text = serde_json::to_string_pretty(&bundle)?;
    Ok(redact::redact(&text, secrets))
}

/// Import a bundle as a read-only PLAYBACK run (no model calls, no new actions).
pub fn import_run(conn: &Connection, bundle_text: &str) -> Result<String> {
    if bundle_text.len() > 50_000_000 {
        return Err(CoreError::Invalid("bundle too large".into()));
    }
    let b: Value = serde_json::from_str(bundle_text)?;
    if b["format"] != EXPORT_FORMAT {
        return Err(CoreError::Invalid("not a RoomLab run export".into()));
    }
    let m: RunManifest = serde_json::from_value(b["manifest"].clone()).map_err(|e| CoreError::Invalid(format!("manifest: {e}")))?;
    let id = new_id("run");
    let mut m2 = m.clone();
    m2.run_id = id.clone();
    m2.mode = RunMode::Playback;
    m2.parent_run_id = Some(m.run_id.clone());
    let max_seq = b["events"].as_array().map(|a| a.iter().filter_map(|e| e["seq"].as_i64()).max().unwrap_or(0)).unwrap_or(0);
    conn.execute(
        "INSERT INTO experiment_runs(id, workspace_id, name, scenario_id, session_type, mode, seed, manifest_json, budget_output_tokens, budget_turns, max_chain_depth, spawn_policy, parent_run_id, created_at, next_seq, paused)
         VALUES (?1,?2,?3,?4,?5,'PLAYBACK',?6,?7,0,0,0,?8,?9,?10,?11,1)",
        params![id, WORKSPACE_ID, format!("{} (imported playback)", m.name), m.scenario_id, m.session_type.as_str(), m.seed, serde_json::to_string(&m2)?, m.spawn_policy.as_str(), m.run_id, now_utc(), max_seq + 1],
    )?;
    let ins = |table: &str, cols: &[&str], rows: &Value| -> Result<()> {
        let Some(arr) = rows.as_array() else { return Ok(()) };
        let placeholders: Vec<String> = (1..=cols.len() + 1).map(|i| format!("?{i}")).collect();
        let sql = format!("INSERT OR IGNORE INTO {table}(run_id, {}) VALUES ({})", cols.join(","), placeholders.join(","));
        let mut st = conn.prepare(&sql)?;
        for r in arr {
            let mut vals: Vec<rusqlite::types::Value> = vec![rusqlite::types::Value::Text(id.clone())];
            for c in cols {
                vals.push(match &r[*c] {
                    Value::Null => rusqlite::types::Value::Null,
                    Value::Number(n) => n.as_i64().map(rusqlite::types::Value::Integer).unwrap_or(rusqlite::types::Value::Real(n.as_f64().unwrap_or(0.0))),
                    Value::String(s) => rusqlite::types::Value::Text(s.clone()),
                    other => rusqlite::types::Value::Text(other.to_string()),
                });
            }
            st.execute(rusqlite::params_from_iter(vals))?;
        }
        Ok(())
    };
    ins("rooms", &["id", "kind", "name", "settings_json", "rule_revision", "archived", "created_at"], &b["rooms"])?;
    ins("channels", &["id", "room_id", "name", "kind"], &b["channels"])?;
    ins("agents", &["id", "display_name", "icon", "kind", "cognition", "model_route", "status", "parent_agent_id", "spawn_type", "soma_mode", "demo_policy", "created_at", "updated_at"], &b["agents"])?;
    ins("memberships", &["room_id", "agent_id", "role", "can_post", "post_scope", "joined_seq"], &b["memberships"])?;
    ins("messages", &["message_id", "room_id", "channel_id", "author_id", "author_kind", "recipient_ids_json", "visibility", "text", "attachments_json", "modality", "source_event_id", "render_status", "reaction_lane_json", "confidence", "is_demo", "model_id", "chain_depth", "seq"], &b["messages"])?;
    ins("events", &["event_id", "seq", "room_id", "sim_time_ms", "wall_time_utc", "event_type", "origin_kind", "origin_id", "audience_scope", "payload_json", "parent_event_ids_json", "schema_version"], &b["events"])?;
    ins("tasks", &["cluster_id", "room_id", "goal", "owner_agent_id", "status", "assignees_json", "priority", "dependencies_json", "verification_criteria_json", "human_action_needed", "notes_json", "updated_seq"], &b["tasks"])?;
    ins("org_edges", &["id", "room_id", "from_id", "to_id", "relation"], &b["org_edges"])?;
    // Imported messages are visible to the human participant view in playback.
    conn.execute(
        "INSERT INTO observations(run_id, observation_id, observer_id, room_id, source_event_id, channel, kind, data_json, status, provenance, observed_at_sim_ms, seq)
         SELECT run_id, 'obs_imp_' || message_id, ?2, room_id, source_event_id, 'ROOM_CHAT', 'MESSAGE', json_object('message_id', message_id), 'OBSERVED', 'import_v1', 0, seq FROM messages WHERE run_id=?1",
        params![id, HUMAN_ID],
    )?;
    Ok(id)
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct PlaybackFrame {
    pub upto_seq: i64,
    pub max_seq: i64,
    pub events: Vec<EventEnvelope>,
    pub message_ids: Vec<String>,
    pub note: String,
}

/// Deterministic playback: reads recorded events only; makes no model calls (T54).
pub fn playback(conn: &Connection, run_id: &str, upto_seq: i64) -> Result<PlaybackFrame> {
    let max_seq = events::current_seq(conn, run_id)?;
    let evs = events::list(conn, run_id, &EventFilter { upto_seq: Some(upto_seq), limit: 20000, ..Default::default() })?;
    let mut st = conn.prepare("SELECT message_id FROM messages WHERE run_id=?1 AND seq <= ?2 ORDER BY seq")?;
    let mids = st.query_map(params![run_id, upto_seq], |r| r.get(0))?.collect::<rusqlite::Result<Vec<String>>>()?;
    Ok(PlaybackFrame { upto_seq, max_seq, events: evs, message_ids: mids, note: "PLAYBACK: recorded events only, no model calls.".into() })
}

const FORK_TABLES: &[&str] = &[
    "rooms", "channels", "agents", "agent_profiles", "memberships", "agent_subscriptions", "messages", "events", "observations", "beliefs",
    "knowledge_resources", "acl_entries", "tasks", "org_edges", "spawn_requests", "spawn_instances", "worlds", "locations", "world_edges",
    "world_entities", "inventories", "intent_envelopes", "runtime_usage", "attachments", "experiment_conditions",
];

/// FORK: clone the run's current checkpoint into a new run id. LLM hidden state is not cloned.
pub fn fork_run(conn: &Connection, run_id: &str, name: &str) -> Result<String> {
    let id = new_id("run");
    let mut m = manifest(conn, run_id)?;
    m.run_id = id.clone();
    m.mode = RunMode::Fork;
    m.parent_run_id = Some(run_id.into());
    m.name = name.into();
    conn.execute(
        "INSERT INTO experiment_runs(id, workspace_id, name, scenario_id, session_type, mode, seed, manifest_json, status, contaminated, next_seq, sim_time_ms, paused, parent_run_id, budget_output_tokens, used_output_tokens, budget_turns, used_turns, max_chain_depth, spawn_policy, created_at)
         SELECT ?2, workspace_id, ?3, scenario_id, session_type, 'FORK', seed, ?4, status, contaminated, next_seq, sim_time_ms, 1, id, budget_output_tokens, used_output_tokens, budget_turns, used_turns, max_chain_depth, spawn_policy, ?5
         FROM experiment_runs WHERE id=?1",
        params![run_id, id, name, serde_json::to_string(&m)?, now_utc()],
    )?;
    for t in FORK_TABLES {
        let cols: Vec<String> = {
            let mut st = conn.prepare(&format!("PRAGMA table_info({t})"))?;
            let v = st.query_map([], |r| r.get::<_, String>(1))?.collect::<rusqlite::Result<Vec<_>>>()?;
            v
        };
        let rest: Vec<&String> = cols.iter().filter(|c| c.as_str() != "run_id").collect();
        let list = rest.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(",");
        conn.execute(&format!("INSERT INTO {t}(run_id,{list}) SELECT ?2,{list} FROM {t} WHERE run_id=?1"), params![run_id, id])?;
    }
    conn.execute(
        "INSERT INTO knowledge_fts(title, body, run_id, resource_id) SELECT title, body, run_id, id FROM knowledge_resources WHERE run_id=?1",
        params![id],
    )?;
    events::append(conn, &id, NewEvent::system(EventType::RunCreated, json!({"forked_from": run_id, "note": "FORK copies state and history; model outputs after this point may diverge."})))?;
    Ok(id)
}

/// Operator-only "who knows what" view (§9, demo script step 4).
pub fn knowledge_matrix(conn: &Connection, run_id: &str) -> Result<Vec<Value>> {
    let agents = crate::agent::list_agents(conn, run_id)?;
    let mut out = vec![];
    for a in agents {
        let mut st = conn.prepare("SELECT kind, COUNT(*) FROM observations WHERE run_id=?1 AND observer_id=?2 GROUP BY kind")?;
        let counts: serde_json::Map<String, Value> = st
            .query_map(params![run_id, a.id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?
            .into_iter()
            .map(|(k, n)| (k, json!(n)))
            .collect();
        let mut st = conn.prepare(
            "SELECT kind, data_json, seq FROM observations WHERE run_id=?1 AND observer_id=?2 AND kind IN ('VISIBLE_OBJECT_STATE','SOUND','REVEAL','ACTION_RESULT','TASK_BRIEF') ORDER BY seq DESC LIMIT 12",
        )?;
        let notable = st
            .query_map(params![run_id, a.id], |r| Ok(json!({"kind": r.get::<_, String>(0)?, "data": serde_json::from_str::<Value>(&r.get::<_, String>(1)?).unwrap_or_default(), "seq": r.get::<_, i64>(2)?})))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let resources: Vec<Value> = crate::knowledge::visible_resources(conn, run_id, &crate::auth::Principal::Identity(a.id.clone()), 50)?
            .into_iter()
            .map(|r| json!({"id": r.id, "title": r.title, "namespace": r.namespace, "kind": r.kind}))
            .collect();
        let mut st = conn.prepare("SELECT claim, status, source_ref FROM beliefs WHERE run_id=?1 AND agent_id=?2")?;
        let beliefs = st
            .query_map(params![run_id, a.id], |r| Ok(json!({"claim": r.get::<_, String>(0)?, "status": r.get::<_, String>(1)?, "source": r.get::<_, String>(2)?})))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        out.push(json!({
            "agent_id": a.id, "display_name": a.display_name, "icon": a.icon, "status": a.status,
            "observation_counts": counts, "notable_observations": notable, "accessible_resources": resources, "beliefs": beliefs
        }));
    }
    Ok(out)
}
