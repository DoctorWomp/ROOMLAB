//! G1/G6 — core, persistence, contracts, events, export/import, playback (T03 T04 T06 T09 T10 T11 T12 T13 T52 T53 T54 T55).
mod common;
use common::*;

use roomlab_core::contracts::*;
use roomlab_core::events::{self, EventFilter};
use roomlab_core::experiments::{self, DirectorChange, ExportKind};
use roomlab_core::{room, Engine, ViewRole};

#[test]
fn t03_room_creation_all_four_presets() {
    let env = engine();
    let (run, _) = sandbox(&env);
    for (k, n) in [(RoomKind::Commons, "c2"), (RoomKind::Guild, "guild"), (RoomKind::Story, "story"), (RoomKind::Crucible, "crucible")] {
        let id = env.e.create_room_from_template(&run, k, n).unwrap();
        let info = env.e.read(|c| room::get_room(c, &run, &id)).unwrap();
        assert_eq!(info.kind, k);
        assert!(info.members.iter().any(|m| m.agent_id == "human_01"));
        match k {
            RoomKind::Guild => assert!(info.members.iter().any(|m| m.role == "coordinator")),
            RoomKind::Story => assert!(info.members.iter().any(|m| m.role == "narrator" && m.post_scope == "NARRATOR")),
            RoomKind::Crucible => {
                for r in ["steelman", "skeptic", "mediator"] {
                    assert!(info.members.iter().any(|m| m.role == r), "missing {r}");
                }
            }
            RoomKind::Commons => assert_eq!(info.members.len(), 4),
        }
    }
}

#[test]
fn t04_message_delivered_once_to_all_and_only_eligible_members() {
    let env = engine();
    let (run, room) = sandbox(&env);
    let guild = env.e.create_room_from_template(&run, RoomKind::Guild, "guild").unwrap();
    let _ = guild;
    let m = env.e.post_human(&run, &room, "hello everyone", AudienceScope::RoomMembers, vec![], vec![]).unwrap();
    let per: Vec<(String, i64)> = env.e.read(|c| {
        let mut st = c.prepare("SELECT observer_id, COUNT(*) FROM observations WHERE run_id=?1 AND kind='MESSAGE' AND json_extract(data_json,'$.message_id')=?2 GROUP BY observer_id ORDER BY observer_id")?;
        let v = st.query_map([&run, &m.record.message_id], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(v)
    }).unwrap();
    let ids: Vec<&str> = per.iter().map(|(a, _)| a.as_str()).collect();
    assert_eq!(ids, vec!["human_01", "lumen", "moth", "sable"]);
    assert!(per.iter().all(|(_, n)| *n == 1), "each delivered exactly once");
    // wren is a guild member only and must not receive the Commons message.
    assert!(!ids.contains(&"wren"));
    // Direct message: only recipient + author.
    let d = env.e.post_human(&run, &room, "just for moth", AudienceScope::Direct, vec!["moth".into()], vec![]).unwrap();
    let obs = count(&env.e, &format!("SELECT COUNT(*) FROM observations WHERE run_id=?1 AND json_extract(data_json,'$.message_id')='{}'", d.record.message_id), &run);
    assert_eq!(obs, 2);
}

#[test]
fn t06_unicode_emoji_rtl_long_strings() {
    let env = engine();
    let (run, room) = sandbox(&env);
    let text = "👩‍👩‍👧‍👦 family ❤️ VS16\nline two\nمرحبا بالعالم (RTL) \u{200F}mark";
    let m = env.e.post_human(&run, &room, text, AudienceScope::RoomMembers, vec![], vec![]).unwrap();
    assert_eq!(m.record.text, text, "UTF-8 round-trips exactly");
    assert_eq!(roomlab_core::soma::emoji_count("👩‍👩‍👧‍👦"), 1, "ZWJ sequence is one grapheme");
    assert_eq!(roomlab_core::soma::emoji_count("❤️"), 1);
    let long: String = "ж".repeat(3999);
    assert!(env.e.post_human(&run, &room, &long, AudienceScope::RoomMembers, vec![], vec![]).is_ok());
    let too_long: String = "a".repeat(4001);
    assert!(env.e.post_human(&run, &room, &too_long, AudienceScope::RoomMembers, vec![], vec![]).is_err());
    // Control characters are stripped, not stored.
    let m2 = env.e.post_human(&run, &room, "bell\u{7}null\u{0}ok", AudienceScope::RoomMembers, vec![], vec![]).unwrap();
    assert_eq!(m2.record.text, "bellnullok");
}

#[test]
fn t09_simple_advanced_switch_does_not_change_permissions() {
    let env = engine();
    let (run, _room) = sandbox(&env);
    let before = env.e.read(|c| roomlab_core::auth::allowed_actions(c, &run, "moth")).unwrap();
    let acl_before = count(&env.e, "SELECT COUNT(*) FROM acl_entries WHERE run_id=?1", &run);
    env.e.update_settings(|s| s.settings_mode = "ADVANCED".into()).unwrap();
    env.e.update_settings(|s| s.settings_mode = "SIMPLE".into()).unwrap();
    assert_eq!(before, env.e.read(|c| roomlab_core::auth::allowed_actions(c, &run, "moth")).unwrap());
    assert_eq!(acl_before, count(&env.e, "SELECT COUNT(*) FROM acl_entries WHERE run_id=?1", &run));
}

#[tokio::test]
async fn t10_crash_restart_keeps_consistent_state() {
    let dir = tempfile::tempdir().unwrap();
    let run;
    {
        let e = Engine::open_temp(dir.path()).unwrap();
        run = e.first_run("DEMO").unwrap();
        let room = e.read(|c| Ok(room::list_rooms(c, &run)?[0].id.clone())).unwrap();
        e.post_human(&run, &room, "before the crash", AudienceScope::RoomMembers, vec![], vec![]).unwrap();
        // Claim a job so it is RUNNING at the moment of the "crash".
        let j = e.tx(|c| roomlab_core::scheduler::claim_next(c, &run)).unwrap();
        assert!(j.is_some());
        // Simulate a forced stop: skip destructors (no clean close / checkpoint).
        std::mem::forget(e);
    }
    let e = Engine::open_temp(dir.path()).unwrap();
    assert!(e.read(|c| roomlab_core::db::integrity_ok(c)).unwrap(), "integrity + FK check after forced stop");
    assert!(count(&e, "SELECT COUNT(*) FROM messages WHERE run_id=?1 AND text='before the crash'", &run) == 1);
    assert_eq!(count(&e, "SELECT COUNT(*) FROM jobs WHERE run_id=?1 AND status='RUNNING'", &run), 0, "no job left RUNNING");
    assert_eq!(count(&e, "SELECT COUNT(*) FROM jobs WHERE run_id=?1 AND outcome='INTERRUPTED_BY_RESTART'", &run), 1);
    assert_eq!(e.settings().active_run_id.as_deref(), Some(run.as_str()), "settings persisted");
}

#[tokio::test]
async fn t11_t12_export_import_roundtrip_and_redaction() {
    let env = engine();
    let (run, room) = sandbox(&env);
    env.e.set_secret("provider", Some("sk-live-SUPERSECRETKEY1234567890".into()));
    env.e.post_human(&run, &room, "public hello", AudienceScope::RoomMembers, vec![], vec![]).unwrap();
    env.e.post_human(&run, &room, "private to moth", AudienceScope::Direct, vec!["moth".into()], vec![]).unwrap();
    env.e.add_knowledge(&run, "lumen", "secret plan", "PRIVATE-PLAN-XYZ", "PRIVATE", None).unwrap();
    drain(&env.e, &run).await;
    let secrets = env.e.secret_values();
    let public = env.e.read(|c| experiments::export_run(c, &run, ExportKind::Public, &secrets)).unwrap();
    let op = env.e.read(|c| experiments::export_run(c, &run, ExportKind::Operator, &secrets)).unwrap();
    for exp in [&public, &op] {
        assert!(!exp.contains("SUPERSECRETKEY"), "no provider credentials in exports");
    }
    assert!(!public.contains("private to moth"));
    assert!(!public.contains("PRIVATE-PLAN-XYZ"));
    assert!(op.contains("private to moth"));
    roomlab_core::redact::log_line(&env.e.paths.logs, &secrets, "calling with Bearer sk-live-SUPERSECRETKEY1234567890");
    let log = std::fs::read_to_string(env.e.paths.logs.join("roomlab.log")).unwrap();
    assert!(!log.contains("SUPERSECRETKEY"));
    // Import → read-only playback run preserving public metadata.
    let imported = env.e.tx(|c| experiments::import_run(c, &public)).unwrap();
    let m = env.e.read(|c| experiments::manifest(c, &imported)).unwrap();
    assert_eq!(m.mode, RunMode::Playback);
    assert_eq!(count(&env.e, "SELECT COUNT(*) FROM messages WHERE run_id=?1 AND text='public hello'", &imported), 1);
    assert_eq!(count(&env.e, "SELECT COUNT(*) FROM messages WHERE run_id=?1 AND text='private to moth'", &imported), 0);
    let r = env.e.human_action(&imported, &room, ActionKind::Speak, Default::default(), Some("hi".into())).unwrap();
    assert!(!r.accepted, "playback runs are read-only");
}

#[test]
fn t13_agent_create_edit_preserves_stable_ids() {
    let env = engine();
    let (run, room) = sandbox(&env);
    for (id, name) in [("alpha", "Alpha"), ("beta", "Beta"), ("gamma", "Gamma")] {
        add_model_agent(&env.e, &run, &room, id, name);
    }
    let mut p = env.e.read(|c| roomlab_core::agent::get_profile(c, &run, "beta")).unwrap();
    p.display_name = "Beta Prime".into();
    p.character.role = "critic".into();
    let v = env.e.tx(|c| roomlab_core::agent::update_agent(c, &run, &p)).unwrap();
    assert_eq!(v, 2);
    let a = env.e.read(|c| roomlab_core::auth::require_agent(c, &run, "beta")).unwrap();
    assert_eq!(a.id, "beta");
    assert_eq!(a.display_name, "Beta Prime");
    // Duplicate id rejected; invalid profile rejected.
    let dup = env.e.read(|c| roomlab_core::agent::get_profile(c, &run, "alpha")).unwrap();
    assert!(env.e.tx(|c| roomlab_core::agent::create_agent(c, &run, &dup, AgentKind::Agent, None, None)).is_err());
}

#[test]
fn t52_run_manifest_contents() {
    let env = engine();
    let (run, _) = sandbox(&env);
    let m = env.e.read(|c| experiments::manifest(c, &run)).unwrap();
    assert_eq!(m.seed, 4201);
    assert_eq!(m.schema_version, "1.0");
    assert!(!m.app_version.is_empty());
    assert_eq!(m.session_type, SessionType::Sandbox);
    assert!(m.model_routes.iter().any(|r| r.route_id == "local/default"));
    assert!(m.budgets.agent_turns > 0);
}

#[tokio::test]
async fn t53_event_sequence_strict_and_causal() {
    let env = engine();
    let (run, room) = sandbox(&env);
    env.e.post_human(&run, &room, "Is this a question?", AudienceScope::RoomMembers, vec![], vec![]).unwrap();
    drain(&env.e, &run).await;
    let evs = env.e.read(|c| events::list(c, &run, &EventFilter { limit: 20000, ..Default::default() })).unwrap();
    assert!(evs.len() > 10);
    for (i, e) in evs.iter().enumerate() {
        assert_eq!(e.seq, i as i64 + 1, "strictly monotonic, gap-free seq");
    }
    let ids: std::collections::HashSet<_> = evs.iter().map(|e| e.event_id.clone()).collect();
    assert_eq!(ids.len(), evs.len(), "no duplicate commits");
    for e in &evs {
        for p in &e.parent_event_ids {
            assert!(ids.contains(p), "parent {p} exists");
            let ps = evs.iter().find(|x| &x.event_id == p).unwrap().seq;
            assert!(ps < e.seq, "causal parent precedes child");
        }
    }
}

#[tokio::test]
async fn t54_playback_makes_no_model_calls_fork_is_new_run() {
    let env = engine();
    let (run, room) = sandbox(&env);
    env.e.post_human(&run, &room, "hello", AudienceScope::RoomMembers, vec![], vec![]).unwrap();
    drain(&env.e, &run).await;
    let calls = count(&env.e, "SELECT COUNT(*) FROM provider_requests WHERE run_id=?1", &run);
    let seq = env.e.read(|c| events::current_seq(c, &run)).unwrap();
    let f = env.e.read(|c| experiments::playback(c, &run, seq / 2)).unwrap();
    assert!(f.events.iter().all(|e| e.seq <= seq / 2));
    assert_eq!(calls, count(&env.e, "SELECT COUNT(*) FROM provider_requests WHERE run_id=?1", &run));
    assert_eq!(seq, env.e.read(|c| events::current_seq(c, &run)).unwrap(), "playback appends nothing");
    let fork = env.e.tx(|c| experiments::fork_run(c, &run, "fork A")).unwrap();
    let fm = env.e.read(|c| experiments::manifest(c, &fork)).unwrap();
    assert_eq!(fm.mode, RunMode::Fork);
    assert_eq!(fm.parent_run_id.as_deref(), Some(run.as_str()));
    assert_eq!(count(&env.e, "SELECT COUNT(*) FROM messages WHERE run_id=?1", &fork), count(&env.e, "SELECT COUNT(*) FROM messages WHERE run_id=?1", &run));
    // The fork is independent.
    env.e.post_human(&fork, &room, "only in fork", AudienceScope::RoomMembers, vec![], vec![]).unwrap();
    assert_eq!(count(&env.e, "SELECT COUNT(*) FROM messages WHERE run_id=?1 AND text='only in fork'", &run), 0);
}

#[test]
fn t55_director_intervention_contaminates_controlled_trial() {
    let env = engine();
    env.e.first_run("DEMO").unwrap();
    let run = env.e.start_scenario("S001_LIBRARY_SECRET", true).unwrap();
    let room = env.e.read(|c| Ok(room::list_rooms(c, &run)?[0].id.clone())).unwrap();
    let ch = DirectorChange::MoveEntity { room_id: room.clone(), entity_id: "lantern".into(), to_location: "cellar".into() };
    let contaminated = env.e.tx(|c| experiments::director_edit(c, &run, &ch, "move lantern", false)).unwrap();
    assert!(contaminated);
    assert_eq!(count(&env.e, "SELECT contaminated FROM experiment_runs WHERE id=?1", &run), 1);
    let (before, after): (String, String) = env.e.read(|c| Ok(c.query_row("SELECT before_json, after_json FROM interventions WHERE run_id=?1", [&run], |r| Ok((r.get(0)?, r.get(1)?)))?)).unwrap();
    assert!(before.contains("great_hall") && after.contains("cellar"));
    // Sandbox: journaled but not contaminated.
    let sb = env.e.start_scenario("S003_HIDDEN_PASSAGE", false).unwrap();
    let r2 = env.e.read(|c| Ok(room::list_rooms(c, &sb)?[0].id.clone())).unwrap();
    let c2 = env.e.tx(|c| experiments::director_edit(c, &sb, &DirectorChange::SetEntityState { room_id: r2, entity_id: "crypt_door".into(), state: "UNLOCKED".into() }, "unlock", false)).unwrap();
    assert!(!c2);
    // Participant → operator switch during a controlled trial requires acknowledgement.
    env.e.set_view_role(Some(&run), ViewRole::Participant, false).unwrap();
    assert!(env.e.set_view_role(Some(&run), ViewRole::Operator, false).is_err());
    assert_eq!(env.e.view_role(), ViewRole::Participant);
    assert!(env.e.set_view_role(Some(&run), ViewRole::Operator, true).unwrap());
}

#[test]
fn contracts_round_trip_and_reject_invalid() {
    let req = roomlab_core::actions::new_request("run_x", "vex-01", 130, ActionKind::Inspect);
    let s = serde_json::to_string(&req).unwrap();
    let back: ActionRequest = serde_json::from_str(&s).unwrap();
    assert_eq!(back, req);
    // Unknown field rejected.
    let mut v: serde_json::Value = serde_json::from_str(&s).unwrap();
    v["hack"] = serde_json::json!(true);
    assert!(serde_json::from_value::<ActionRequest>(v).is_err());
    // Unknown enum rejected.
    let bad = s.replace("\"INSPECT\"", "\"LAUNCH_MISSILES\"");
    assert!(serde_json::from_str::<ActionRequest>(&bad).is_err());
    // Spec examples round-trip.
    let spawn = r#"{"schema_version":"1.0","request_id":"spawn_req_24","run_id":"run_01","parent_agent_id":"lumen","spawn_type":"TASK_WORKER","display_name":"Counterexample Scout","requested_model_route":"room_default_subagent","objective":"Identify two counterexamples to the proposal","requested_visibility":"PARENT_ONLY","context_handoff_refs":["task_brief_07"],"requested_actions":["SPEAK","NO_OP"],"requested_budget":{"max_turns":8,"max_output_tokens_total":1000,"ttl_seconds":300}}"#;
    let sr: SpawnRequest = serde_json::from_str(spawn).unwrap();
    assert_eq!(serde_json::from_str::<SpawnRequest>(&serde_json::to_string(&sr).unwrap()).unwrap(), sr);
    let obs = r#"{"observation_id":"obs_140","observer_id":"vex-01","source_event_id":"evt_130","channel":"WORLD_LOCAL","kind":"VISIBLE_OBJECT_STATE","data":{"object":"library_shelf","feature":"clean_rectangle"},"status":"OBSERVED","provenance":"world_visibility_gate_v1","observed_at_sim_ms":9820,"expires_at_sim_ms":null}"#;
    let o: Observation = serde_json::from_str(obs).unwrap();
    assert_eq!(serde_json::from_str::<Observation>(&serde_json::to_string(&o).unwrap()).unwrap(), o);
    assert!(serde_json::from_str::<Observation>(&obs.replace("OBSERVED", "PROBABLY")).is_err());
    let world: WorldSpec = serde_json::from_str(roomlab_core::scenarios::FORGOTTEN_KEEP).unwrap();
    assert!(roomlab_core::world::validate_spec(&world).is_ok());
    let mut bad_world = world.clone();
    bad_world.locations[0].exits.push("nowhere".into());
    assert!(roomlab_core::world::validate_spec(&bad_world).is_err());
    // Provider config never serializes an API key.
    let pc = ProviderConfig { api_key: Some("sk-should-not-serialize".into()), ..Default::default() };
    assert!(!serde_json::to_string(&pc).unwrap().contains("sk-should"));
}

#[test]
fn t63_unwritable_portable_folder_falls_back_with_visible_notice() {
    let bogus = std::path::Path::new(r"Q:\definitely\not\a\writable\folder");
    let p = roomlab_core::settings::DataPaths::resolve(bogus, None).unwrap();
    assert!(!p.portable);
    assert!(p.notice.as_deref().unwrap_or("").contains("not writable"));
    assert!(roomlab_core::settings::is_writable(&p.data));
}
