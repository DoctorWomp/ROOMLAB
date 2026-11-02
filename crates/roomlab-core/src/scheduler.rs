//! Event-driven turn scheduler (§8.3–8.4). Identity residency ≠ queued job ≠ active generation.
//! - Cheap eligibility rules (mention / direct / human prompt / task) — no LLM call to decide.
//! - Max one QUEUED job per agent (coalescing) and one RUNNING job per agent.
//! - Priority with fairness aging; chain-depth cap stops agent↔agent ping-pong (T17).
//! - Run budgets are checked before work is queued and charged after (T25).

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::auth;
use crate::comms::{self, Posted};
use crate::contracts::*;
use crate::error::Result;
use crate::events::{self, NewEvent};
use crate::ids::{new_id, now_ms};
use crate::room;

pub const PRIO_HUMAN_DIRECT: i64 = 0;
pub const PRIO_TASK: i64 = 1;
pub const PRIO_GROUP: i64 = 2;
pub const PRIO_AMBIENT: i64 = 3;
/// Default job deadline: queued work older than this becomes STALE rather than running late.
pub const JOB_DEADLINE_MS: i64 = 10 * 60 * 1000;

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Job {
    pub job_id: String,
    pub run_id: String,
    pub agent_id: String,
    pub room_id: String,
    pub trigger_event_id: String,
    pub trigger_message_id: Option<String>,
    pub observation_cursor: i64,
    pub revision: i64,
    pub priority: i64,
    pub token_ceiling: i64,
    pub chain_depth: i64,
    pub reason: String,
    pub enqueued_seq: i64,
    pub enqueued_wall_ms: i64,
    pub deadline_wall_ms: i64,
}

pub fn log_turn(conn: &Connection, run_id: &str, agent_id: &str, room_id: Option<&str>, trigger: Option<&str>, outcome: TurnOutcome, detail: &str) -> Result<()> {
    let seq = events::current_seq(conn, run_id)?;
    conn.execute(
        "INSERT INTO turn_log(run_id, id, agent_id, room_id, trigger_event_id, outcome, detail, seq) VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
        params![run_id, new_id("turn"), agent_id, room_id, trigger, outcome.as_str(), detail, seq],
    )?;
    Ok(())
}

#[derive(Clone, Debug)]
pub struct RunBudgetState {
    pub budget_tokens: i64,
    pub used_tokens: i64,
    pub budget_turns: i64,
    pub used_turns: i64,
    pub max_chain_depth: i64,
    pub paused: bool,
}

impl RunBudgetState {
    pub fn tokens_left(&self) -> i64 {
        (self.budget_tokens - self.used_tokens).max(0)
    }
    pub fn turns_left(&self) -> i64 {
        (self.budget_turns - self.used_turns).max(0)
    }
}

pub fn budget_state(conn: &Connection, run_id: &str) -> Result<RunBudgetState> {
    Ok(conn.query_row(
        "SELECT budget_output_tokens, used_output_tokens, budget_turns, used_turns, max_chain_depth, paused FROM experiment_runs WHERE id=?1",
        params![run_id],
        |r| {
            Ok(RunBudgetState {
                budget_tokens: r.get(0)?,
                used_tokens: r.get(1)?,
                budget_turns: r.get(2)?,
                used_turns: r.get(3)?,
                max_chain_depth: r.get(4)?,
                paused: r.get::<_, i64>(5)? != 0,
            })
        },
    )?)
}

/// Post-call accounting. Usage is recorded as measured; remaining budget saturates at zero.
pub fn charge(conn: &Connection, run_id: &str, agent_id: &str, prompt_tokens: i64, output_tokens: i64, is_demo: bool) -> Result<()> {
    let out = output_tokens.max(0);
    conn.execute(
        "UPDATE experiment_runs SET used_turns = used_turns + 1, used_output_tokens = used_output_tokens + ?2 WHERE id=?1",
        params![run_id, out],
    )?;
    conn.execute("INSERT OR IGNORE INTO runtime_usage(run_id, agent_id) VALUES (?1,?2)", params![run_id, agent_id])?;
    if is_demo {
        conn.execute("UPDATE runtime_usage SET demo_turns = demo_turns + 1 WHERE run_id=?1 AND agent_id=?2", params![run_id, agent_id])?;
    } else {
        conn.execute(
            "UPDATE runtime_usage SET calls = calls + 1, prompt_tokens = prompt_tokens + ?3, output_tokens = output_tokens + ?4 WHERE run_id=?1 AND agent_id=?2",
            params![run_id, agent_id, prompt_tokens.max(0), out],
        )?;
    }
    // Child budget accounting (spawn instances).
    conn.execute(
        "UPDATE spawn_instances SET turns_used = turns_used + 1, tokens_used = tokens_used + ?3 WHERE run_id=?1 AND child_agent_id=?2",
        params![run_id, agent_id, out],
    )?;
    Ok(())
}

pub struct EnqueueSpec<'a> {
    pub agent_id: &'a str,
    pub room_id: &'a str,
    pub trigger_event_id: &'a str,
    pub trigger_message_id: Option<&'a str>,
    pub priority: i64,
    pub chain_depth: i64,
    pub reason: &'a str,
}

pub fn enqueue(conn: &Connection, run_id: &str, s: EnqueueSpec) -> Result<Option<String>> {
    let Some(agent) = auth::load_agent(conn, run_id, s.agent_id)? else { return Ok(None) };
    let room = Some(s.room_id);
    let trig = Some(s.trigger_event_id);
    if agent.kind == AgentKind::Human {
        return Ok(None);
    }
    if agent.status != AgentStatus::Active {
        log_turn(conn, run_id, s.agent_id, room, trig, TurnOutcome::SuppressedByPolicy, &format!("identity {}", agent.status.as_str()))?;
        return Ok(None);
    }
    let b = budget_state(conn, run_id)?;
    if s.chain_depth > b.max_chain_depth {
        log_turn(conn, run_id, s.agent_id, room, trig, TurnOutcome::SuppressedByPolicy, &format!("autonomous chain depth {} > cap {}", s.chain_depth, b.max_chain_depth))?;
        return Ok(None);
    }
    if b.turns_left() == 0 || (agent.cognition.uses_model() && b.tokens_left() == 0) {
        log_turn(conn, run_id, s.agent_id, room, trig, TurnOutcome::SuppressedByPolicy, "BUDGET_EXHAUSTED")?;
        return Ok(None);
    }
    // Child turn/TTL caps.
    let child: Option<(i64, i64, i64, i64, String)> = conn
        .query_row(
            "SELECT turns_used, max_turns, tokens_used, max_tokens, state FROM spawn_instances WHERE run_id=?1 AND child_agent_id=?2",
            params![run_id, s.agent_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .optional()?;
    if let Some((tu, mt, toku, mtok, state)) = &child {
        if state != "ACTIVE" || tu >= mt || toku >= mtok {
            log_turn(conn, run_id, s.agent_id, room, trig, TurnOutcome::SuppressedByPolicy, "child budget/turn cap or not active")?;
            return Ok(None);
        }
    }
    // Coalesce: at most one QUEUED job per agent.
    let existing: Option<(String, i64)> = conn
        .query_row(
            "SELECT job_id, priority FROM jobs WHERE run_id=?1 AND agent_id=?2 AND status='QUEUED'",
            params![run_id, s.agent_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    let seq = events::current_seq(conn, run_id)?;
    if let Some((jid, prio)) = existing {
        conn.execute(
            "UPDATE jobs SET trigger_event_id=?3, trigger_message_id=?4, observation_cursor=?5, priority=?6, chain_depth=?7, room_id=?8, reason=?9 WHERE run_id=?1 AND job_id=?2",
            params![run_id, jid, s.trigger_event_id, s.trigger_message_id, seq, prio.min(s.priority), s.chain_depth, s.room_id, s.reason],
        )?;
        log_turn(conn, run_id, s.agent_id, room, trig, TurnOutcome::Coalesced, &format!("merged into {jid}"))?;
        return Ok(Some(jid));
    }
    let token_ceiling = crate::agent::get_profile(conn, run_id, s.agent_id)
        .map(|p| p.limits.turn_output_tokens)
        .unwrap_or(200)
        .min(b.tokens_left().max(1));
    let revision: i64 = conn
        .query_row("SELECT rule_revision FROM rooms WHERE run_id=?1 AND id=?2", params![run_id, s.room_id], |r| r.get(0))
        .optional()?
        .unwrap_or(1);
    let job_id = new_id("job");
    let now = now_ms();
    conn.execute(
        "INSERT INTO jobs(run_id, job_id, agent_id, room_id, trigger_event_id, trigger_message_id, observation_cursor, revision, priority, token_ceiling, status, chain_depth, reason, enqueued_seq, enqueued_wall_ms, deadline_wall_ms)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,'QUEUED',?11,?12,?13,?14,?15)",
        params![run_id, job_id, s.agent_id, s.room_id, s.trigger_event_id, s.trigger_message_id, seq, revision, s.priority, token_ceiling, s.chain_depth, s.reason, seq, now, now + JOB_DEADLINE_MS],
    )?;
    events::append(
        conn,
        run_id,
        NewEvent::new(EventType::JobQueued, OriginKind::System, "scheduler", AudienceScope::OperatorOnly, json!({
            "job_id": job_id, "agent_id": s.agent_id, "priority": s.priority, "reason": s.reason, "chain_depth": s.chain_depth
        }))
        .room(s.room_id)
        .parents(vec![s.trigger_event_id.to_string()]),
    )?;
    log_turn(conn, run_id, s.agent_id, room, trig, TurnOutcome::Queued, s.reason)?;
    Ok(Some(job_id))
}

/// Cheap eligibility after a message is delivered. Returns queued job ids.
pub fn on_message(conn: &Connection, run_id: &str, posted: &Posted) -> Result<Vec<String>> {
    let m = &posted.message;
    let kind = room::room_kind(conn, run_id, &m.room_id)?;
    let mentions = comms::parse_mentions(conn, run_id, &m.room_id, &m.text)?;
    let everyone = mentions.iter().any(|x| x == "*");
    let author_is_human = m.author_kind == OriginKind::Human;
    let depth: i64 = conn.query_row(
        "SELECT chain_depth FROM messages WHERE run_id=?1 AND message_id=?2",
        params![run_id, m.message_id],
        |r| r.get(0),
    )?;
    let mut jobs = vec![];
    for rid in &posted.delivered_to {
        if rid == &m.author_id {
            continue;
        }
        let Some(a) = auth::load_agent(conn, run_id, rid)? else { continue };
        if matches!(a.kind, AgentKind::Human | AgentKind::Narrator) {
            continue;
        }
        let mentioned = mentions.contains(rid) || everyone;
        let direct = matches!(m.visibility, AudienceScope::Direct | AudienceScope::ParentOnly);
        let is_child = a.parent_agent_id.is_some();
        let from_parent = a.parent_agent_id.as_deref() == Some(m.author_id.as_str());
        let (eligible, prio, why) = if is_child {
            (from_parent || mentioned, PRIO_TASK, "parent instruction")
        } else if direct {
            (true, if author_is_human { PRIO_HUMAN_DIRECT } else { PRIO_TASK }, "direct message")
        } else if mentioned {
            (true, if author_is_human { PRIO_HUMAN_DIRECT } else { PRIO_GROUP }, "mentioned")
        } else if a.cognition == Cognition::L1Reactive {
            // L1 REACTIVE: model invoked only on mention / direct / task boundary (§8.3).
            (false, PRIO_AMBIENT, "L1 reactive: not mentioned")
        } else if author_is_human && matches!(kind, RoomKind::Commons | RoomKind::Guild) && m.author_kind != OriginKind::World {
            (true, PRIO_GROUP, "group turn offered (human message)")
        } else {
            (false, PRIO_AMBIENT, "not addressed")
        };
        if !eligible {
            log_turn(conn, run_id, rid, Some(&m.room_id), Some(&m.source_event_id), TurnOutcome::SuppressedByPolicy, why)?;
            continue;
        }
        log_turn(conn, run_id, rid, Some(&m.room_id), Some(&m.source_event_id), TurnOutcome::OfferedTurn, why)?;
        let next_depth = if author_is_human { 1 } else { depth + 1 };
        if let Some(j) = enqueue(conn, run_id, EnqueueSpec {
            agent_id: rid,
            room_id: &m.room_id,
            trigger_event_id: &m.source_event_id,
            trigger_message_id: Some(&m.message_id),
            priority: prio,
            chain_depth: next_depth,
            reason: why,
        })? {
            jobs.push(j);
        }
    }
    Ok(jobs)
}

fn row_job(r: &rusqlite::Row) -> rusqlite::Result<Job> {
    Ok(Job {
        job_id: r.get("job_id")?,
        run_id: r.get("run_id")?,
        agent_id: r.get("agent_id")?,
        room_id: r.get("room_id")?,
        trigger_event_id: r.get("trigger_event_id")?,
        trigger_message_id: r.get("trigger_message_id")?,
        observation_cursor: r.get("observation_cursor")?,
        revision: r.get("revision")?,
        priority: r.get("priority")?,
        token_ceiling: r.get("token_ceiling")?,
        chain_depth: r.get("chain_depth")?,
        reason: r.get("reason")?,
        enqueued_seq: r.get("enqueued_seq")?,
        enqueued_wall_ms: r.get("enqueued_wall_ms")?,
        deadline_wall_ms: r.get("deadline_wall_ms")?,
    })
}

/// Claim the next job, honouring pause/step. Expired jobs go STALE.
pub fn claim_next(conn: &Connection, run_id: &str) -> Result<Option<Job>> {
    let (paused, step_tokens): (i64, i64) = conn.query_row(
        "SELECT paused, step_tokens FROM experiment_runs WHERE id=?1",
        params![run_id],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    if paused != 0 && step_tokens <= 0 {
        return Ok(None);
    }
    let now = now_ms();
    conn.execute(
        "UPDATE jobs SET status='STALE', outcome='DEADLINE_PASSED', finished_wall_ms=?2 WHERE run_id=?1 AND status='QUEUED' AND deadline_wall_ms < ?2",
        params![run_id, now],
    )?;
    // Budgets are re-checked at claim time: exhausted budgets cancel queued work (no ghost requests).
    let b = budget_state(conn, run_id)?;
    if b.turns_left() == 0 {
        cancel_queued(conn, run_id, "BUDGET_EXHAUSTED")?;
        return Ok(None);
    }
    if b.tokens_left() == 0 {
        conn.execute(
            "UPDATE jobs SET status='CANCELLED', outcome='BUDGET_EXHAUSTED', finished_wall_ms=?2 WHERE run_id=?1 AND status='QUEUED'
             AND agent_id IN (SELECT id FROM agents WHERE run_id=?1 AND cognition != 'L0_SCRIPTED')",
            params![run_id, now],
        )?;
    }
    let cur = events::current_seq(conn, run_id)?;
    let job = conn
        .query_row(
            "SELECT j.* FROM jobs j LEFT JOIN runtime_usage u ON u.run_id=j.run_id AND u.agent_id=j.agent_id
             WHERE j.run_id=?1 AND j.status='QUEUED'
               AND NOT EXISTS (SELECT 1 FROM jobs k WHERE k.run_id=j.run_id AND k.agent_id=j.agent_id AND k.status='RUNNING')
             ORDER BY (j.priority * 20 - MIN(?2 - j.enqueued_seq, 240) + MIN(COALESCE(u.calls,0) + COALESCE(u.demo_turns,0), 10)) ASC, j.enqueued_seq ASC
             LIMIT 1",
            params![run_id, cur],
            row_job,
        )
        .optional()?;
    if let Some(j) = &job {
        conn.execute(
            "UPDATE jobs SET status='RUNNING', started_wall_ms=?3 WHERE run_id=?1 AND job_id=?2",
            params![run_id, j.job_id, now],
        )?;
        if paused != 0 {
            conn.execute("UPDATE experiment_runs SET step_tokens = MAX(step_tokens - 1, 0) WHERE id=?1", params![run_id])?;
        }
    }
    Ok(job)
}

pub fn finish(conn: &Connection, run_id: &str, job_id: &str, status: &str, outcome: &str) -> Result<()> {
    conn.execute(
        "UPDATE jobs SET status=?3, outcome=?4, finished_wall_ms=?5 WHERE run_id=?1 AND job_id=?2",
        params![run_id, job_id, status, outcome, now_ms()],
    )?;
    events::append(
        conn,
        run_id,
        NewEvent::new(EventType::JobFinished, OriginKind::System, "scheduler", AudienceScope::OperatorOnly, json!({"job_id": job_id, "status": status, "outcome": outcome})),
    )?;
    Ok(())
}

pub fn set_paused(conn: &Connection, run_id: &str, paused: bool) -> Result<()> {
    conn.execute("UPDATE experiment_runs SET paused=?2, step_tokens=0 WHERE id=?1", params![run_id, paused as i64])?;
    events::append(conn, run_id, NewEvent::new(if paused { EventType::RunPaused } else { EventType::RunResumed }, OriginKind::Human, crate::HUMAN_ID, AudienceScope::OperatorOnly, json!({})))?;
    Ok(())
}

pub fn step(conn: &Connection, run_id: &str) -> Result<()> {
    conn.execute("UPDATE experiment_runs SET paused=1, step_tokens = step_tokens + 1 WHERE id=?1", params![run_id])?;
    events::append(conn, run_id, NewEvent::new(EventType::RunStepped, OriginKind::Human, crate::HUMAN_ID, AudienceScope::OperatorOnly, json!({})))?;
    Ok(())
}

pub fn cancel_queued(conn: &Connection, run_id: &str, reason: &str) -> Result<i64> {
    Ok(conn.execute(
        "UPDATE jobs SET status='CANCELLED', outcome=?2, finished_wall_ms=?3 WHERE run_id=?1 AND status='QUEUED'",
        params![run_id, reason, now_ms()],
    )? as i64)
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct RuntimeStatus {
    pub run_id: String,
    pub run_name: String,
    pub session_type: String,
    pub mode: String,
    pub contaminated: bool,
    pub paused: bool,
    pub identities_active: i64,
    pub identities_dormant: i64,
    pub children_active: i64,
    pub queued_jobs: i64,
    pub running_jobs: i64,
    pub failed_jobs: i64,
    pub spawn_pending_approval: i64,
    pub used_turns: i64,
    pub budget_turns: i64,
    pub used_output_tokens: i64,
    pub budget_output_tokens: i64,
    pub last_seq: i64,
    pub sim_time_ms: i64,
    pub errors: i64,
}

pub fn runtime_status(conn: &Connection, run_id: &str) -> Result<RuntimeStatus> {
    let mut s = conn.query_row(
        "SELECT name, session_type, mode, contaminated, paused, used_turns, budget_turns, used_output_tokens, budget_output_tokens, next_seq-1, sim_time_ms FROM experiment_runs WHERE id=?1",
        params![run_id],
        |r| {
            Ok(RuntimeStatus {
                run_id: run_id.to_string(),
                run_name: r.get(0)?,
                session_type: r.get(1)?,
                mode: r.get(2)?,
                contaminated: r.get::<_, i64>(3)? != 0,
                paused: r.get::<_, i64>(4)? != 0,
                used_turns: r.get(5)?,
                budget_turns: r.get(6)?,
                used_output_tokens: r.get(7)?,
                budget_output_tokens: r.get(8)?,
                last_seq: r.get(9)?,
                sim_time_ms: r.get(10)?,
                ..Default::default()
            })
        },
    )?;
    let count = |sql: &str| -> Result<i64> { Ok(conn.query_row(sql, params![run_id], |r| r.get(0))?) };
    s.identities_active = count("SELECT COUNT(*) FROM agents WHERE run_id=?1 AND status='ACTIVE' AND kind!='HUMAN'")?;
    s.identities_dormant = count("SELECT COUNT(*) FROM agents WHERE run_id=?1 AND status='DORMANT'")?;
    s.children_active = count("SELECT COUNT(*) FROM spawn_instances WHERE run_id=?1 AND state IN ('INITIALIZING','ACTIVE','SUSPENDED')")?;
    s.queued_jobs = count("SELECT COUNT(*) FROM jobs WHERE run_id=?1 AND status='QUEUED'")?;
    s.running_jobs = count("SELECT COUNT(*) FROM jobs WHERE run_id=?1 AND status='RUNNING'")?;
    s.failed_jobs = count("SELECT COUNT(*) FROM jobs WHERE run_id=?1 AND status='FAILED'")?;
    s.spawn_pending_approval = count("SELECT COUNT(*) FROM spawn_requests WHERE run_id=?1 AND state='AWAIT_APPROVAL'")?;
    s.errors = count("SELECT COUNT(*) FROM events WHERE run_id=?1 AND event_type IN ('INFERENCE_FAILED')")?;
    Ok(s)
}
