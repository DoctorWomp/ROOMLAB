//! Job execution (§8.4): claim → (L0 policy | authorized context → model stream) →
//! parse/validate (≤1 bounded repair) → commit → account. Streaming drafts are reported
//! to the UI but nothing is durable until the action gate accepts the final output.

use std::sync::atomic::Ordering;

use rusqlite::params;
use serde::Serialize;
use serde_json::json;

use crate::actions::{self, ActionContext, ParseOutcome};
use crate::auth;
use crate::comms::{self, PostSpec};
use crate::context::{self, ChatMsg};
use crate::contracts::*;
use crate::crucible;
use crate::demo::{self, DemoInput};
use crate::error::{CoreError, Result};
use crate::events::{self, NewEvent};
use crate::ids::{estimate_tokens, new_id, now_ms};
use crate::providers::{self, openai_compat};
use crate::room;
use crate::scheduler::{self, Job};
use crate::spawn;
use crate::Engine;

#[derive(Serialize, Clone, Debug)]
pub struct DraftUpdate {
    pub run_id: String,
    pub job_id: String,
    pub agent_id: String,
    pub agent_name: String,
    pub room_id: String,
    pub text: String,
    pub done: bool,
}

#[derive(Serialize, Clone, Debug)]
pub struct JobReport {
    pub job_id: String,
    pub agent_id: String,
    pub room_id: String,
    pub outcome: String,
    pub model_id: Option<String>,
    pub accepted: Vec<bool>,
    pub detail: String,
}

impl Engine {
    fn latest_message_since(&self, run_id: &str, agent: &str, room: &str, seq: i64) -> Option<String> {
        self.read(|c| {
            Ok(c.query_row(
                "SELECT message_id FROM messages WHERE run_id=?1 AND author_id=?2 AND room_id=?3 AND seq > ?4 ORDER BY seq DESC LIMIT 1",
                params![run_id, agent, room, seq],
                |r| r.get::<_, String>(0),
            )
            .ok())
        })
        .ok()
        .flatten()
    }

    fn close_job(&self, run_id: &str, job: &Job, outcome: TurnOutcome, status: &str, detail: &str, before_seq: i64) -> Result<()> {
        let produced = self.latest_message_since(run_id, &job.agent_id, &job.room_id, before_seq);
        self.tx(|c| {
            scheduler::log_turn(c, run_id, &job.agent_id, Some(&job.room_id), Some(&job.trigger_event_id), outcome, detail)?;
            scheduler::finish(c, run_id, &job.job_id, status, detail)?;
            events::advance_sim_clock(c, run_id, 1000)?;
            if status == "DONE" {
                crucible::advance(c, run_id, job, produced.as_deref())?;
            }
            Ok(())
        })
    }

    /// Execute a job for an L0 scripted agent. No model calls; outputs labelled DEMO.
    pub fn execute_demo_job(&self, run_id: &str, job: &Job) -> Result<JobReport> {
        let before = self.read(|c| events::current_seq(c, run_id))?;
        let decisions = self.read(|c| {
            let agent = auth::require_agent(c, run_id, &job.agent_id)?;
            let kind = room::room_kind(c, run_id, &job.room_id)?;
            let trigger = match &job.trigger_message_id {
                Some(m) => comms::get_message(c, run_id, m).ok(),
                None => None,
            };
            demo::decide(c, &DemoInput { run_id, job, agent: &agent, room_kind: kind, trigger, seq: before })
        })?;
        let ctx = ActionContext { room_id: job.room_id.clone(), chain_depth: job.chain_depth, is_demo: true, model_id: None, trigger_event_id: Some(job.trigger_event_id.clone()) };
        let mut accepted = vec![];
        let mut details = vec![];
        for d in &decisions {
            let r = self.submit_action(run_id, d, &ctx)?;
            accepted.push(r.accepted);
            details.push(format!("{} {}", d.action.as_str(), if r.accepted { "ok".to_string() } else { r.reason_code.as_str() }));
        }
        self.tx(|c| scheduler::charge(c, run_id, &job.agent_id, 0, 0, true))?;
        let (outcome, detail) = if decisions.is_empty() {
            (TurnOutcome::ExplicitNoop, "scripted silence (NO_OP)".to_string())
        } else {
            (TurnOutcome::Generated, details.join("; "))
        };
        if decisions.is_empty() {
            self.tx(|c| {
                events::append(c, run_id, NewEvent::new(EventType::ActionExecuted, OriginKind::Agent, &job.agent_id, AudienceScope::OperatorOnly, json!({"action": "NO_OP", "is_demo": true, "detail": "scripted silence"})).room(&job.room_id))?;
                Ok(())
            })?;
        }
        self.close_job(run_id, job, outcome, "DONE", &detail, before)?;
        Ok(JobReport { job_id: job.job_id.clone(), agent_id: job.agent_id.clone(), room_id: job.room_id.clone(), outcome: outcome.as_str(), model_id: None, accepted, detail })
    }

    fn fail_model_job(&self, run_id: &str, job: &Job, agent_name: &str, why: &str, cancel_others: bool) -> Result<JobReport> {
        let before = self.read(|c| events::current_seq(c, run_id))?;
        self.tx(|c| {
            events::append(c, run_id, NewEvent::new(EventType::InferenceFailed, OriginKind::System, "gateway", AudienceScope::OperatorOnly, json!({"job_id": job.job_id, "agent_id": job.agent_id, "error": why})).room(&job.room_id))?;
            let mut n = 0;
            if cancel_others {
                n = c.execute(
                    "UPDATE jobs SET status='CANCELLED', outcome='MODEL_UNAVAILABLE', finished_wall_ms=?2 WHERE run_id=?1 AND status='QUEUED'
                     AND agent_id IN (SELECT id FROM agents WHERE run_id=?1 AND cognition != 'L0_SCRIPTED')",
                    params![run_id, now_ms()],
                )?;
            }
            comms::post(c, run_id, PostSpec {
                room_id: job.room_id.clone(),
                author_id: "gateway".into(),
                text: format!(
                    "⚠ MODEL_UNAVAILABLE — {agent_name} could not get a model reply: {why}{} No response was invented. Fix the provider in Settings, or switch this agent to a Demo policy.",
                    if n > 0 { format!(" {n} other queued model job(s) were cancelled.") } else { String::new() }
                ),
                visibility: Some(AudienceScope::OperatorOnly),
                system_origin: Some(OriginKind::System),
                chain_depth: 99,
                ..Default::default()
            })?;
            Ok(())
        })?;
        self.close_job(run_id, job, TurnOutcome::Failed, "FAILED", &format!("MODEL_UNAVAILABLE: {why}"), before)?;
        Ok(JobReport { job_id: job.job_id.clone(), agent_id: job.agent_id.clone(), room_id: job.room_id.clone(), outcome: "FAILED".into(), model_id: None, accepted: vec![], detail: why.into() })
    }

    /// Claim and execute the next job. Returns None when idle, paused, or in playback.
    pub async fn process_next(&self, run_id: &str, on_draft: &mut (dyn FnMut(DraftUpdate) + Send)) -> Result<Option<JobReport>> {
        self.tx(|c| spawn::enforce_limits(c, run_id).map(|_| ()))?;
        let mode: String = self.read(|c| Ok(c.query_row("SELECT mode FROM experiment_runs WHERE id=?1", [run_id], |r| r.get(0))?))?;
        if mode == "PLAYBACK" {
            return Ok(None);
        }
        let Some(job) = self.tx(|c| scheduler::claim_next(c, run_id))? else { return Ok(None) };
        let agent = self.read(|c| auth::require_agent(c, run_id, &job.agent_id))?;
        if agent.status != AgentStatus::Active {
            let before = self.read(|c| events::current_seq(c, run_id))?;
            self.close_job(run_id, &job, TurnOutcome::SuppressedByPolicy, "CANCELLED", "identity not active", before)?;
            return Ok(Some(JobReport { job_id: job.job_id, agent_id: job.agent_id, room_id: job.room_id, outcome: "CANCELLED".into(), model_id: None, accepted: vec![], detail: "identity not active".into() }));
        }
        if !agent.cognition.uses_model() {
            return self.execute_demo_job(run_id, &job).map(Some);
        }
        self.run_model_job(run_id, &job, &agent, on_draft).await.map(Some)
    }

    async fn run_model_job(&self, run_id: &str, job: &Job, agent: &auth::AgentRow, on_draft: &mut (dyn FnMut(DraftUpdate) + Send)) -> Result<JobReport> {
        self.cancel_flag.store(false, Ordering::SeqCst);
        let s = self.settings();
        let (managed_base, managed_key) = {
            let mut g = self.managed.lock().await;
            match g.as_mut() {
                Some(m) => {
                    if m.is_running() { (Some(m.base_url()), Some(m.api_key.clone())) } else { (None, None) }
                }
                None => (None, None),
            }
        };
        let key = if s.provider.provider_kind == ProviderKind::ManagedLocal { managed_key } else { self.secret("provider") };
        let target = match providers::resolve(&agent.model_route, &s.provider, key, managed_base) {
            Ok(t) => t,
            Err(e) => {
                let msg = match &e {
                    CoreError::Rejected { message, .. } => message.clone(),
                    other => other.to_string(),
                };
                return self.fail_model_job(run_id, job, &agent.display_name, &msg, false);
            }
        };
        let prompt = self.read(|c| context::build(c, run_id, &agent.id, &job.room_id, Some(job)))?;
        let plain_ok = self.read(|c| room::room_settings(c, run_id, &job.room_id))?.plain_text_as_speak;
        let preq_id = new_id("preq");
        let started = now_ms();
        let enqueued_at = job.enqueued_wall_ms.max(0);
        let host = url::Url::parse(&target.endpoint.base_url).ok().and_then(|u| u.host_str().map(|h| format!("{h}:{}", u.port_or_known_default().unwrap_or(0))));
        let max_tokens = job.token_ceiling + target.reasoning_allowance;
        let sampling = json!({"temperature": target.temperature, "max_tokens": max_tokens, "visible_ceiling": job.token_ceiling, "reasoning_allowance": target.reasoning_allowance});
        self.tx(|c| {
            c.execute(
                "INSERT INTO provider_requests(run_id, id, job_id, agent_id, provider_kind, endpoint_host, requested_model, prompt_hash, context_refs_json, sampling_json, prompt_tokens_est, wall_start_ms, queue_wait_ms, status)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,'STARTED')",
                params![run_id, preq_id, job.job_id, agent.id, target.kind.as_str(), host, target.model_id, prompt.template_hash, serde_json::to_string(&prompt.resource_refs)?, sampling.to_string(), prompt.token_estimate, started, started - enqueued_at],
            )?;
            Ok(())
        })?;
        let gen = openai_compat::GenRequest { model: target.model_id.clone(), messages: prompt.messages.clone(), max_tokens, temperature: target.temperature };
        let mut acc = String::new();
        let base = DraftUpdate { run_id: run_id.into(), job_id: job.job_id.clone(), agent_id: agent.id.clone(), agent_name: agent.display_name.clone(), room_id: job.room_id.clone(), text: String::new(), done: false };
        let res = {
            let mut cb = |d: &str| {
                acc.push_str(d);
                on_draft(DraftUpdate { text: acc.clone(), ..base.clone() });
            };
            openai_compat::chat_stream(&target.endpoint, &gen, &self.cancel_flag, &mut cb).await
        };
        on_draft(DraftUpdate { done: true, ..base.clone() });
        let mut res = match res {
            Ok(r) => r,
            Err(e) => {
                self.tx(|c| {
                    c.execute("UPDATE provider_requests SET status='FAILED', error=?3, wall_end_ms=?4 WHERE run_id=?1 AND id=?2", params![run_id, preq_id, e.to_string(), now_ms()])?;
                    Ok(())
                })?;
                return self.fail_model_job(run_id, job, &agent.display_name, &e.to_string(), true);
            }
        };
        if res.cancelled {
            let before = self.read(|c| events::current_seq(c, run_id))?;
            self.tx(|c| {
                c.execute("UPDATE provider_requests SET status='CANCELLED', wall_end_ms=?3 WHERE run_id=?1 AND id=?2", params![run_id, preq_id, now_ms()])?;
                Ok(())
            })?;
            self.close_job(run_id, job, TurnOutcome::Failed, "CANCELLED", "generation cancelled by operator; nothing committed", before)?;
            return Ok(JobReport { job_id: job.job_id.clone(), agent_id: agent.id.clone(), room_id: job.room_id.clone(), outcome: "CANCELLED".into(), model_id: None, accepted: vec![], detail: "cancelled".into() });
        }
        let reasoning_exhausted = res.text.trim().is_empty() && res.finish_reason.as_deref() == Some("length");
        if reasoning_exhausted {
            let why = format!(
                "the model used its whole output budget ({} tokens{}) without a visible reply. Thinking models (e.g. Gemma 4, Qwen3) need a Reasoning allowance — set ~800–1500 in Settings → Inference.",
                res.usage.completion_tokens.map(|c| c.to_string()).unwrap_or_else(|| "?".into()),
                res.usage.reasoning_tokens.map(|r| format!(", {r} hidden reasoning")).unwrap_or_default()
            );
            self.tx(|c| {
                comms::post(c, run_id, PostSpec {
                    room_id: job.room_id.clone(),
                    author_id: "gateway".into(),
                    text: format!("⚠ {}: {why} Recorded as NO_OP; nothing was invented.", agent.display_name),
                    visibility: Some(AudienceScope::OperatorOnly),
                    system_origin: Some(OriginKind::System),
                    chain_depth: 99,
                    ..Default::default()
                })?;
                c.execute("UPDATE provider_requests SET error='REASONING_EXHAUSTED_BUDGET' WHERE run_id=?1 AND id=?2", params![run_id, preq_id])?;
                Ok(())
            })?;
        }
        let mut parsed = actions::parse_model_output(&res.text, plain_ok);
        let mut repaired = false;
        if let ParseOutcome::Invalid(why) = &parsed {
            // Exactly one bounded repair request (§13.5).
            repaired = true;
            let mut msgs = prompt.messages.clone();
            msgs.push(ChatMsg { role: "assistant".into(), content: res.text.chars().take(2000).collect() });
            msgs.push(ChatMsg { role: "user".into(), content: format!("That reply was not a valid RoomLab action ({why}). Reply again with ONLY one JSON object of the required shape.") });
            if let Ok(r2) = openai_compat::chat_once(&target.endpoint, &openai_compat::GenRequest { model: target.model_id.clone(), messages: msgs, max_tokens: job.token_ceiling.min(300) + target.reasoning_allowance, temperature: target.temperature }).await {
                parsed = actions::parse_model_output(&r2.text, plain_ok);
                res.usage.completion_tokens = Some(res.usage.completion_tokens.unwrap_or(0) + r2.usage.completion_tokens.unwrap_or_else(|| estimate_tokens(&r2.text)));
                res.text = format!("{}\n--- repair ---\n{}", res.text, r2.text);
            }
        }
        let resolved_model = res.model.clone().unwrap_or_else(|| target.model_id.clone());
        let ctx = ActionContext { room_id: job.room_id.clone(), chain_depth: job.chain_depth, is_demo: false, model_id: Some(resolved_model.clone()), trigger_event_id: Some(job.trigger_event_id.clone()) };
        let before = self.read(|c| events::current_seq(c, run_id))?;
        let (result, parse_status) = match parsed {
            ParseOutcome::Action(a) => (Some(self.submit_action(run_id, &actions::to_request(run_id, &agent.id, job.observation_cursor, a), &ctx)?), "ACTION"),
            ParseOutcome::PlainSpeech(t) => {
                let mut r = actions::new_request(run_id, &agent.id, job.observation_cursor, ActionKind::Speak);
                r.message_text = Some(t);
                (Some(self.submit_action(run_id, &r, &ctx)?), "PLAIN_SPEECH")
            }
            ParseOutcome::Empty => (None, "EMPTY_NOOP"),
            ParseOutcome::Invalid(why) => {
                // Fail closed: journal INVALID_SCHEMA, execute nothing (NO_OP).
                let mut r = actions::new_request(run_id, &agent.id, job.observation_cursor, ActionKind::NoOp);
                r.args.insert("raw_excerpt".into(), json!(res.text.chars().take(300).collect::<String>()));
                let rr = self.tx(|c| actions::record_rejection(c, run_id, &r, &ctx, ReasonCode::InvalidSchema, &format!("unparseable model output after repair: {why}")))?;
                (Some(rr), "INVALID_NOOP")
            }
        };
        let prompt_tokens = res.usage.prompt_tokens.unwrap_or(prompt.token_estimate);
        let out_tokens = res.usage.completion_tokens.unwrap_or_else(|| estimate_tokens(&res.text));
        let usage_measured = res.usage.completion_tokens.is_some();
        self.tx(|c| {
            scheduler::charge(c, run_id, &agent.id, prompt_tokens, out_tokens, false)?;
            let committed = result.as_ref().and_then(|r| r.result_event_ids.first().cloned());
            c.execute(
                "UPDATE provider_requests SET resolved_model=?3, usage_json=?4, wall_end_ms=?5, first_token_ms=?6, status=?7, committed_event_id=?8, repair_attempted=?9 WHERE run_id=?1 AND id=?2",
                params![run_id, preq_id, resolved_model, json!({"prompt_tokens": res.usage.prompt_tokens, "completion_tokens": res.usage.completion_tokens, "reasoning_tokens": res.usage.reasoning_tokens, "finish_reason": res.finish_reason, "estimated": !usage_measured}).to_string(), now_ms(), res.first_token_ms, parse_status, committed, repaired as i64],
            )?;
            events::append(c, run_id, NewEvent::new(EventType::InferenceCompleted, OriginKind::System, "gateway", AudienceScope::OperatorOnly, json!({
                "provider_request_id": preq_id, "agent_id": agent.id, "provider_kind": target.kind, "endpoint_host": host,
                "requested_model": target.model_id, "resolved_model": resolved_model, "prompt_template_hash": prompt.template_hash,
                "room_rule_revision": prompt.rule_revision, "context_refs": prompt.resource_refs, "sampling": sampling,
                "context_token_estimate": prompt.token_estimate, "usage": {"prompt_tokens": res.usage.prompt_tokens, "completion_tokens": res.usage.completion_tokens, "reasoning_tokens": res.usage.reasoning_tokens},
                "first_token_ms": res.first_token_ms, "total_ms": res.total_ms, "finish_reason": res.finish_reason, "parse": parse_status, "repair_attempted": repaired,
                "accepted": result.as_ref().map(|r| r.accepted)
            })).room(&job.room_id))?;
            Ok(())
        })?;
        let accepted = result.as_ref().map(|r| vec![r.accepted]).unwrap_or_default();
        let detail = format!("{parse_status}; {}", result.as_ref().and_then(|r| r.detail.clone()).unwrap_or_default());
        let outcome = if parse_status == "EMPTY_NOOP" || parse_status == "INVALID_NOOP" { TurnOutcome::ExplicitNoop } else { TurnOutcome::Generated };
        self.close_job(run_id, job, outcome, "DONE", &detail, before)?;
        Ok(JobReport { job_id: job.job_id.clone(), agent_id: agent.id.clone(), room_id: job.room_id.clone(), outcome: outcome.as_str(), model_id: Some(resolved_model), accepted, detail })
    }
}
