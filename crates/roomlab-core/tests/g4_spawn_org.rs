//! G4 — Spawn Governor and Guild org (T30 T31 T32 T33 T35).
mod common;
use common::*;

use roomlab_core::actions::{new_request, ActionContext};
use roomlab_core::auth::Principal;
use roomlab_core::contracts::*;
use roomlab_core::{context, knowledge, org, room, spawn};

fn guild(env: &TestEnv) -> (String, String) {
    let (run, _) = sandbox(env);
    let g = env.e.create_room_from_template(&run, RoomKind::Guild, "guild").unwrap();
    (run, g)
}

fn req_spawn(env: &TestEnv, run: &str, room: &str, parent: &str, name: &str, refs: Vec<String>) -> ActionResult {
    let mut r = new_request(run, parent, 0, ActionKind::RequestSpawn);
    r.args = serde_json::from_value(serde_json::json!({"display_name": name, "objective": "Find two counterexamples.", "context_handoff_refs": refs})).unwrap();
    env.e.submit_action(run, &r, &ActionContext { room_id: room.into(), ..Default::default() }).unwrap()
}

fn pending(env: &TestEnv, run: &str) -> Vec<String> {
    env.e.read(|c| Ok(spawn::list(c, run)?.into_iter().filter(|s| s.state == "AWAIT_APPROVAL").map(|s| s.request_id).collect())).unwrap()
}

#[tokio::test]
async fn t30_t31_child_gets_explicit_handoff_only_after_approval() {
    let env = engine();
    let (run, g) = guild(&env);
    let brief: String = env.e.read(|c| Ok(c.query_row("SELECT id FROM knowledge_resources WHERE run_id=?1 AND title='Task brief notes'", [&run], |r| r.get(0))?)).unwrap();
    let hidden: String = env.e.read(|c| Ok(c.query_row("SELECT id FROM knowledge_resources WHERE run_id=?1 AND title LIKE 'Coordinator private%'", [&run], |r| r.get(0))?)).unwrap();
    let agents_before = count(&env.e, "SELECT COUNT(*) FROM agents WHERE run_id=?1", &run);
    let r = req_spawn(&env, &run, &g, "lumen", "Scout", vec![brief.clone()]);
    assert!(r.accepted);
    // T31: request is visible to the operator; no child exists until approval.
    let p = pending(&env, &run);
    assert_eq!(p.len(), 1);
    assert_eq!(count(&env.e, "SELECT COUNT(*) FROM agents WHERE run_id=?1", &run), agents_before);
    let d = env.e.tx(|c| spawn::decide(c, &run, &p[0], true)).unwrap();
    assert_eq!(d.state, SpawnState::Active);
    let child = d.child_agent_id.unwrap();
    // T30: explicit handoff only.
    let cp = Principal::Identity(child.clone());
    assert!(env.e.read(|c| knowledge::read_resource(c, &run, &cp, &brief)).is_ok());
    assert!(env.e.read(|c| knowledge::read_resource(c, &run, &cp, &hidden)).is_err());
    let prompt = env.e.read(|c| context::build(c, &run, &child, &g, None)).unwrap();
    let all: String = prompt.messages.iter().map(|m| m.content.clone()).collect();
    assert!(!all.contains("budget worries") && !all.contains("Do not share"));
    assert!(all.contains("Task brief"), "brief delivered");
    // Child can only post to its parent.
    let mut s = new_request(&run, &child, 0, ActionKind::Speak);
    s.message_text = Some("hello room?".into());
    env.e.submit_action(&run, &s, &ActionContext { room_id: g.clone(), ..Default::default() }).unwrap();
    let vis: String = env.e.read(|c| Ok(c.query_row("SELECT visibility FROM messages WHERE run_id=?1 AND author_id=?2 ORDER BY seq DESC LIMIT 1", [&run, &child], |r| r.get(0))?)).unwrap();
    assert_eq!(vis, "PARENT_ONLY");
    // Denied requests never create children.
    let r2 = req_spawn(&env, &run, &g, "lumen", "Second", vec![]);
    assert!(r2.accepted);
    let p2 = pending(&env, &run);
    env.e.tx(|c| spawn::decide(c, &run, &p2[0], false)).unwrap();
    assert_eq!(count(&env.e, "SELECT COUNT(*) FROM agents WHERE run_id=?1 AND display_name='Second'", &run), 0);
}

#[test]
fn t32_caps_depth_and_budget() {
    let env = engine();
    let (run, g) = guild(&env);
    // Per-parent cap (2): third request rejected.
    for n in ["S1", "S2"] {
        assert!(req_spawn(&env, &run, &g, "lumen", n, vec![]).accepted);
    }
    let r3 = req_spawn(&env, &run, &g, "lumen", "S3", vec![]);
    assert_eq!(r3.reason_code, ReasonCode::SpawnCapReached);
    // Approve both; room cap (3) with a second parent → 4th active child is QUEUED.
    for id in pending(&env, &run) {
        env.e.tx(|c| spawn::decide(c, &run, &id, true)).unwrap();
    }
    env.e.tx(|c| {
        let mut p = roomlab_core::agent::get_profile(c, &run, "wren")?;
        p.autonomy.allowed_actions.push(ActionKind::RequestSpawn);
        roomlab_core::agent::update_agent(c, &run, &p).map(|_| ())
    }).unwrap();
    assert!(req_spawn(&env, &run, &g, "wren", "W1", vec![]).accepted);
    assert!(req_spawn(&env, &run, &g, "wren", "W2", vec![]).accepted);
    let ps = pending(&env, &run);
    let d1 = env.e.tx(|c| spawn::decide(c, &run, &ps[0], true)).unwrap();
    let d2 = env.e.tx(|c| spawn::decide(c, &run, &ps[1], true)).unwrap();
    let states = [d1.state, d2.state];
    assert!(states.contains(&SpawnState::Active) && states.contains(&SpawnState::Queued), "{states:?}");
    assert_eq!(count(&env.e, "SELECT COUNT(*) FROM spawn_instances WHERE run_id=?1 AND state='ACTIVE'", &run), 3);
    // Grandchild denied.
    let child: String = env.e.read(|c| Ok(c.query_row("SELECT child_agent_id FROM spawn_instances WHERE run_id=?1 AND state='ACTIVE' LIMIT 1", [&run], |r| r.get(0))?)).unwrap();
    let mut r = new_request(&run, &child, 0, ActionKind::RequestSpawn);
    r.args = serde_json::from_value(serde_json::json!({"display_name": "grandchild", "objective": "x"})).unwrap();
    let res = env.e.submit_action(&run, &r, &ActionContext { room_id: g.clone(), ..Default::default() }).unwrap();
    assert!(matches!(res.reason_code, ReasonCode::SpawnDepthExceeded | ReasonCode::CapabilityDenied), "{:?}", res.reason_code);
    let res2 = env.e.tx(|c| spawn::request(c, &run, &g, &SpawnRequest {
        schema_version: "1.0".into(), request_id: "gc".into(), run_id: run.clone(), parent_agent_id: child.clone(), spawn_type: SpawnType::TaskWorker,
        display_name: "gc".into(), requested_model_route: "demo".into(), objective: "x".into(), requested_visibility: SpawnVisibility::ParentOnly,
        context_handoff_refs: vec![], requested_actions: vec![ActionKind::Speak], requested_budget: SpawnBudget { max_turns: 1, max_output_tokens_total: 10, ttl_seconds: 10 },
    }, true));
    assert_eq!(res2.unwrap_err().reason_code(), Some(ReasonCode::SpawnDepthExceeded), "even operator path cannot create a grandchild");
    // Budgets are clamped to room caps (intersection).
    let eff: String = env.e.read(|c| Ok(c.query_row("SELECT effective_json FROM spawn_requests WHERE run_id=?1 LIMIT 1", [&run], |r| r.get(0))?)).unwrap();
    let v: serde_json::Value = serde_json::from_str(&eff).unwrap();
    assert!(v["max_turns"].as_i64().unwrap() <= 8 && v["ttl_seconds"].as_i64().unwrap() <= 300);
    // Over-budget (run turns exhausted) → BUDGET_EXHAUSTED.
    env.e.tx(|c| { c.execute("UPDATE experiment_runs SET budget_turns=used_turns WHERE id=?1", [&run])?; Ok(()) }).unwrap();
    let mut rr = new_request(&run, "lumen", 0, ActionKind::RequestSpawn);
    rr.args = serde_json::from_value(serde_json::json!({"display_name": "late", "objective": "x"})).unwrap();
    let late = env.e.tx(|c| roomlab_core::actions::apply_in_tx(c, &run, &rr, &ActionContext { room_id: g.clone(), ..Default::default() }));
    let code = late.unwrap_err().reason_code();
    assert!(matches!(code, Some(ReasonCode::BudgetExhausted) | Some(ReasonCode::SpawnCapReached)), "{code:?}");
}

#[tokio::test]
async fn t33_guild_task_lifecycle_child_reports_and_slot_released() {
    let env = engine();
    env.e.first_run("DEMO").unwrap();
    let run = env.e.start_scenario("G001_ROLE_ALLOCATION", false).unwrap();
    let g = env.e.read(|c| Ok(room::list_rooms(c, &run)?[0].id.clone())).unwrap();
    drain(&env.e, &run).await;
    // Coordinator assigned a task cluster and requested a scout (awaiting approval).
    let tasks = env.e.read(|c| org::list_tasks(c, &run, &g)).unwrap();
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].assignees, vec!["moth".to_string(), "sable".to_string()]);
    let p = pending(&env, &run);
    assert_eq!(p.len(), 1, "scout requested");
    env.e.tx(|c| spawn::decide(c, &run, &p[0], true)).unwrap();
    drain(&env.e, &run).await;
    let rows = env.e.read(|c| spawn::list(c, &run)).unwrap();
    let scout = rows.iter().find(|r| r.display_name == "Counterexample Scout").unwrap();
    assert_eq!(scout.state, "ARCHIVED");
    assert_eq!(scout.end_reason.as_deref(), Some("REPORT_FINAL"));
    let child = scout.child_agent_id.clone().unwrap();
    // Child read the handed-off brief via the tool broker, then reported privately to the parent.
    assert!(count(&env.e, &format!("SELECT COUNT(*) FROM tool_results WHERE run_id=?1 AND agent_id='{child}' AND tool='READ_RESOURCE'"), &run) == 1);
    let (vis, rec): (String, String) = env.e.read(|c| Ok(c.query_row("SELECT visibility, recipient_ids_json FROM messages WHERE run_id=?1 AND author_id=?2 AND text LIKE '[FINAL REPORT]%'", [&run, &child], |r| Ok((r.get(0)?, r.get(1)?)))?)).unwrap();
    assert_eq!(vis, "PARENT_ONLY");
    assert_eq!(rec, "[\"lumen\"]");
    // Other members never observed the child's report.
    assert_eq!(count(&env.e, &format!("SELECT COUNT(*) FROM observations o JOIN messages m ON m.run_id=o.run_id AND json_extract(o.data_json,'$.message_id')=m.message_id WHERE o.run_id=?1 AND m.author_id='{child}' AND o.observer_id IN ('moth','sable','wren','human_01')"), &run), 0);
    assert_eq!(count(&env.e, "SELECT COUNT(*) FROM spawn_instances WHERE run_id=?1 AND state='ACTIVE'", &run), 0, "slot released");
    let t = env.e.read(|c| org::get_task(c, &run, &tasks[0].cluster_id)).unwrap();
    assert_eq!(t.status, TaskStatus::ReadyForReview);
    // Only the human verifies.
    let mut up = new_request(&run, "lumen", 0, ActionKind::UpdateTask);
    up.args = serde_json::from_value(serde_json::json!({"cluster_id": t.cluster_id, "status": "VERIFIED"})).unwrap();
    assert_eq!(env.e.submit_action(&run, &up, &ActionContext { room_id: g.clone(), ..Default::default() }).unwrap().reason_code, ReasonCode::CapabilityDenied);
    let ok = env.e.human_action(&run, &g, ActionKind::UpdateTask, serde_json::from_value(serde_json::json!({"cluster_id": t.cluster_id, "status": "VERIFIED"})).unwrap(), None).unwrap();
    assert!(ok.accepted);
}

#[test]
fn t35_org_rank_does_not_grant_permissions() {
    let env = engine();
    let (run, g) = guild(&env);
    // Elevate Sable in the org graph: reports_to human, can_delegate, can_nominate.
    env.e.tx(|c| {
        org::add_edge(c, &run, &g, "sable", "human_01", OrgRelation::ReportsTo, "human_01")?;
        org::add_edge(c, &run, &g, "sable", "moth", OrgRelation::CanDelegate, "human_01")?;
        org::add_edge(c, &run, &g, "sable", "wren", OrgRelation::CanNominate, "human_01")?;
        Ok(())
    }).unwrap();
    // No spawn authority.
    assert_eq!(req_spawn(&env, &run, &g, "sable", "x", vec![]).reason_code, ReasonCode::CapabilityDenied);
    // No RAG access to the coordinator's notes.
    let s = Principal::Identity("sable".into());
    assert!(env.e.read(|c| knowledge::search(c, &run, &s, "budget worries cut quarter", 10)).unwrap().is_empty());
    // No shell.
    let mut t = new_request(&run, "sable", 0, ActionKind::Tool);
    t.args = serde_json::from_value(serde_json::json!({"tool": "RUN_SHELL"})).unwrap();
    assert_eq!(env.e.submit_action(&run, &t, &ActionContext { room_id: g.clone(), ..Default::default() }).unwrap().reason_code, ReasonCode::UnsafeToolDisabled);
    // Delegation (an org action) requires both the capability and the edge.
    let mut a = new_request(&run, "moth", 0, ActionKind::AssignTask);
    a.args = serde_json::from_value(serde_json::json!({"goal": "x", "assignees": ["sable"]})).unwrap();
    assert_eq!(env.e.submit_action(&run, &a, &ActionContext { room_id: g.clone(), ..Default::default() }).unwrap().reason_code, ReasonCode::CapabilityDenied);
}
