//! G2 — agent runtime and scheduler (T14 T15 T16 T17 T18 T25 T26 T27), demo conversation.
mod common;
use common::*;

use roomlab_core::contracts::*;
use roomlab_core::scheduler::{self, EnqueueSpec};
use roomlab_core::{agent, context, events, room, Engine};

#[tokio::test]
async fn demo_three_agents_converse_without_provider() {
    let env = engine();
    let (run, room) = sandbox(&env);
    env.e.post_human(&run, &room, "Is pineapple on pizza defensible?", AudienceScope::RoomMembers, vec![], vec![]).unwrap();
    let reports = drain(&env.e, &run).await;
    assert!(reports.len() >= 3);
    let msgs = messages_text(&env.e, &run, &room);
    assert!(msgs.iter().any(|(a, _)| a == "lumen"));
    assert!(msgs.iter().any(|(a, _)| a == "moth"));
    // All agent messages are labelled DEMO, no provider calls happened.
    assert_eq!(count(&env.e, "SELECT COUNT(*) FROM messages WHERE run_id=?1 AND author_kind='AGENT' AND is_demo=0", &run), 0);
    assert_eq!(count(&env.e, "SELECT COUNT(*) FROM provider_requests WHERE run_id=?1", &run), 0);
    // Sable chose explicit silence and it is logged as such (not as a personality result of no offer).
    assert!(count(&env.e, "SELECT COUNT(*) FROM turn_log WHERE run_id=?1 AND agent_id='sable' AND outcome='OFFERED_TURN'", &run) >= 1);
    assert!(count(&env.e, "SELECT COUNT(*) FROM turn_log WHERE run_id=?1 AND agent_id='sable' AND outcome='EXPLICIT_NOOP'", &run) >= 1);
}

#[test]
fn t14_independent_contexts_on_shared_route() {
    let env = engine();
    let (run, room) = sandbox(&env);
    add_model_agent(&env.e, &run, &room, "ada", "Ada");
    add_model_agent(&env.e, &run, &room, "bo", "Bo");
    env.e.add_knowledge(&run, "ada", "Ada private memo", "ADA-ONLY-CONTENT about the launch", "PRIVATE", None).unwrap();
    env.e.post_human(&run, &room, "@Ada what do you remember about the launch?", AudienceScope::RoomMembers, vec![], vec![]).unwrap();
    let pa = env.e.read(|c| context::build(c, &run, "ada", &room, None)).unwrap();
    let pb = env.e.read(|c| context::build(c, &run, "bo", &room, None)).unwrap();
    let priv_a = &pa.segments.iter().find(|s| s.label.starts_with("PRIVATE")).unwrap().text;
    let priv_b = &pb.segments.iter().find(|s| s.label.starts_with("PRIVATE")).unwrap().text;
    assert_ne!(priv_a, priv_b);
    assert!(priv_a.contains("Ada private memo"));
    let all_b: String = pb.messages.iter().map(|m| m.content.clone()).collect();
    assert!(!all_b.contains("Ada private memo") && !all_b.contains("ADA-ONLY-CONTENT"));
    // Same stable system template (shared prefix) for both.
    assert_eq!(pa.template_hash, pb.template_hash);
}

#[tokio::test]
async fn t15_mention_triggers_reply_irrelevant_event_noop_without_model_call() {
    let env = engine();
    let (run, room) = sandbox(&env);
    add_model_agent(&env.e, &run, &room, "ada", "Ada");
    // Not mentioned: L1 reactive agent is not offered a turn, no model call, no job.
    env.e.post_human(&run, &room, "general chatter about the weather", AudienceScope::RoomMembers, vec![], vec![]).unwrap();
    assert_eq!(count(&env.e, "SELECT COUNT(*) FROM jobs WHERE run_id=?1 AND agent_id='ada'", &run), 0);
    assert!(count(&env.e, "SELECT COUNT(*) FROM turn_log WHERE run_id=?1 AND agent_id='ada' AND outcome='SUPPRESSED_BY_POLICY'", &run) >= 1);
    drain(&env.e, &run).await;
    assert_eq!(count(&env.e, "SELECT COUNT(*) FROM provider_requests WHERE run_id=?1", &run), 0);
    // Mentioned: queued.
    env.e.post_human(&run, &room, "@Ada are you there?", AudienceScope::RoomMembers, vec![], vec![]).unwrap();
    assert_eq!(count(&env.e, "SELECT COUNT(*) FROM jobs WHERE run_id=?1 AND agent_id='ada' AND status='QUEUED'", &run), 1);
}

#[test]
fn t16_queue_fairness_aging_prevents_starvation() {
    let env = engine();
    let (run, room) = sandbox(&env);
    let e = &env.e;
    let ev = |e: &Engine| e.tx(|c| Ok(events::append(c, &run, events::NewEvent::system(EventType::SystemNotice, serde_json::json!({})))?.event_id)).unwrap();
    // Sable gets a low-priority job once; Lumen is continuously re-queued at top priority.
    let t = ev(e);
    e.tx(|c| scheduler::enqueue(c, &run, EnqueueSpec { agent_id: "sable", room_id: &room, trigger_event_id: &t, trigger_message_id: None, priority: scheduler::PRIO_AMBIENT, chain_depth: 0, reason: "low" })).unwrap();
    let mut sable_ran_at = None;
    for step in 0..40 {
        let t = ev(e);
        e.tx(|c| scheduler::enqueue(c, &run, EnqueueSpec { agent_id: "lumen", room_id: &room, trigger_event_id: &t, trigger_message_id: None, priority: scheduler::PRIO_HUMAN_DIRECT, chain_depth: 0, reason: "hot" })).unwrap();
        for _ in 0..4 {
            ev(e); // other activity ages queued work
        }
        let j = e.tx(|c| scheduler::claim_next(c, &run)).unwrap().unwrap();
        e.tx(|c| scheduler::finish(c, &run, &j.job_id, "DONE", "test")).unwrap();
        if j.agent_id == "sable" {
            sable_ran_at = Some(step);
            break;
        }
    }
    let at = sable_ran_at.expect("low-priority job starved");
    assert!(at < 30, "ran at step {at}");
}

#[tokio::test]
async fn t17_infinite_reply_stimulus_stops_at_chain_cap() {
    let env = engine();
    let (run, room) = sandbox(&env);
    env.e.tx(|c| {
        for (id, n) in [("echo_a", "EchoA"), ("echo_b", "EchoB")] {
            let p = agent::profile_from_seed(&agent::ProfileSeed { id, name: n, icon: "🔁", role: "echo", persona: "", directive: "", cognition: Cognition::L0Scripted, route: "demo", soma: SomaMode::Off, demo_policy: Some("echo"), actions: agent::CHAT_ACTIONS.to_vec(), motivation: "", style: "" });
            agent::create_agent(c, &run, &p, AgentKind::Agent, None, None)?;
            room::add_member(c, &run, &room, id, "member", "ROOM")?;
        }
        Ok(())
    }).unwrap();
    // Echo agents reply with "@<author> echo" — they mention each other forever without a cap.
    env.e.post_human(&run, &room, "@EchoA @EchoB start", AudienceScope::RoomMembers, vec![], vec![]).unwrap();
    // Make the echo agents address each other: EchoA's reply mentions the human; seed a cross-mention.
    let mut r = roomlab_core::actions::new_request(&run, "echo_a", 0, ActionKind::Speak);
    r.message_text = Some("@EchoB ping".into());
    env.e.submit_action(&run, &r, &roomlab_core::actions::ActionContext { room_id: room.clone(), chain_depth: 1, ..Default::default() }).unwrap();
    let reports = drain(&env.e, &run).await;
    let max_depth = count(&env.e, "SELECT MAX(chain_depth) FROM messages WHERE run_id=?1 AND author_id LIKE 'echo_%' AND chain_depth < 99", &run);
    assert!(reports.len() < 60, "terminated after {} jobs", reports.len());
    assert!(max_depth <= 7, "chain depth capped (got {max_depth})");
    assert!(count(&env.e, "SELECT COUNT(*) FROM turn_log WHERE run_id=?1 AND detail LIKE 'autonomous chain depth%'", &run) >= 1);
}

#[tokio::test]
async fn t18_pause_step_resume() {
    let env = engine();
    let (run, room) = sandbox(&env);
    env.e.tx(|c| scheduler::set_paused(c, &run, true)).unwrap();
    env.e.post_human(&run, &room, "Is anyone there?", AudienceScope::RoomMembers, vec![], vec![]).unwrap();
    let queued = count(&env.e, "SELECT COUNT(*) FROM jobs WHERE run_id=?1 AND status='QUEUED'", &run);
    assert_eq!(queued, 3);
    let mut sink = |_d: roomlab_core::runner::DraftUpdate| {};
    assert!(env.e.process_next(&run, &mut sink).await.unwrap().is_none(), "paused: nothing advances");
    env.e.tx(|c| scheduler::step(c, &run)).unwrap();
    assert!(env.e.process_next(&run, &mut sink).await.unwrap().is_some(), "step processes one");
    assert!(env.e.process_next(&run, &mut sink).await.unwrap().is_none(), "then paused again");
    assert_eq!(count(&env.e, "SELECT COUNT(*) FROM jobs WHERE run_id=?1 AND status='QUEUED'", &run), 2);
    env.e.tx(|c| scheduler::set_paused(c, &run, false)).unwrap();
    drain(&env.e, &run).await;
    assert_eq!(count(&env.e, "SELECT COUNT(*) FROM jobs WHERE run_id=?1 AND status='QUEUED'", &run), 0);
}

#[tokio::test]
async fn t25_budget_exhaustion_stops_new_work() {
    let env = engine();
    let (run, room) = sandbox(&env);
    env.e.tx(|c| {
        c.execute("UPDATE experiment_runs SET budget_turns = used_turns + 2 WHERE id=?1", [&run])?;
        Ok(())
    }).unwrap();
    env.e.post_human(&run, &room, "Is anyone there?", AudienceScope::RoomMembers, vec![], vec![]).unwrap();
    drain(&env.e, &run).await;
    let (used, budget): (i64, i64) = env.e.read(|c| Ok(c.query_row("SELECT used_turns, budget_turns FROM experiment_runs WHERE id=?1", [&run], |r| Ok((r.get(0)?, r.get(1)?)))?)).unwrap();
    assert!(used <= budget, "used {used} ≤ budget {budget}");
    assert!(count(&env.e, "SELECT COUNT(*) FROM jobs WHERE run_id=?1 AND outcome='BUDGET_EXHAUSTED'", &run) >= 1);
    env.e.post_human(&run, &room, "Another?", AudienceScope::RoomMembers, vec![], vec![]).unwrap();
    assert_eq!(count(&env.e, "SELECT COUNT(*) FROM jobs WHERE run_id=?1 AND status='QUEUED'", &run), 0, "no new work queued");
    assert!(count(&env.e, "SELECT COUNT(*) FROM turn_log WHERE run_id=?1 AND detail='BUDGET_EXHAUSTED'", &run) >= 1);
    let b = env.e.read(|c| scheduler::budget_state(c, &run)).unwrap();
    assert_eq!(b.turns_left(), 0);
}

#[tokio::test]
async fn t26_dormant_identity_wakes_with_history_and_no_background_generation() {
    let env = engine();
    let (run, room) = sandbox(&env);
    env.e.tx(|c| agent::set_status(c, &run, "lumen", AgentStatus::Dormant, "test")).unwrap();
    env.e.post_human(&run, &room, "Lumen, you were asleep for this?", AudienceScope::RoomMembers, vec![], vec![]).unwrap();
    drain(&env.e, &run).await;
    assert_eq!(count(&env.e, "SELECT COUNT(*) FROM jobs WHERE run_id=?1 AND agent_id='lumen'", &run), 0, "no generation while dormant");
    assert_eq!(count(&env.e, "SELECT COUNT(*) FROM messages WHERE run_id=?1 AND author_id='lumen'", &run), 0);
    env.e.wake_agent(&run, "lumen", &room, "what did you miss?").unwrap();
    let p = env.e.read(|c| context::build(c, &run, "lumen", &room, None)).unwrap();
    assert!(p.messages[1].content.contains("you were asleep for this"), "history reconstructed from delivered observations");
    drain(&env.e, &run).await;
    assert!(count(&env.e, "SELECT COUNT(*) FROM messages WHERE run_id=?1 AND author_id='lumen'", &run) >= 1);
}

#[test]
fn t27_residency_telemetry_distinguishes_identity_queue_running() {
    let env = engine();
    let (run, room) = sandbox(&env);
    env.e.tx(|c| scheduler::set_paused(c, &run, true)).unwrap();
    env.e.post_human(&run, &room, "Question?", AudienceScope::RoomMembers, vec![], vec![]).unwrap();
    let s = env.e.read(|c| scheduler::runtime_status(c, &run)).unwrap();
    assert_eq!(s.identities_active, 3);
    assert_eq!(s.queued_jobs, 3);
    assert_eq!(s.running_jobs, 0);
    assert!(s.paused);
    env.e.tx(|c| scheduler::step(c, &run)).unwrap();
    env.e.tx(|c| scheduler::claim_next(c, &run)).unwrap();
    let s = env.e.read(|c| scheduler::runtime_status(c, &run)).unwrap();
    assert_eq!((s.queued_jobs, s.running_jobs), (2, 1));
}
