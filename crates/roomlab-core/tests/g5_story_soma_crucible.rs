//! G5 — Story world, SOMA, Crucible (T39–T50).
mod common;
use common::*;

use roomlab_core::actions::{new_request, ActionContext};
use roomlab_core::contracts::*;
use roomlab_core::narrative::{self, NarrationPacket};
use roomlab_core::{context, crucible, room, soma, world, Engine};

fn args(v: serde_json::Value) -> serde_json::Map<String, serde_json::Value> {
    serde_json::from_value(v).unwrap()
}

fn story(env: &TestEnv, scenario: &str) -> (String, String) {
    env.e.first_run("DEMO").unwrap();
    let run = env.e.start_scenario(scenario, false).unwrap();
    let room = env.e.read(|c| Ok(room::list_rooms(c, &run)?[0].id.clone())).unwrap();
    (run, room)
}

fn loc(e: &Engine, run: &str, room: &str, who: &str) -> String {
    e.read(|c| {
        let w = world::world_for_room(c, run, room)?;
        Ok(world::char_location(c, run, &w.world_id, who)?.unwrap_or_default())
    })
    .unwrap()
}

#[test]
fn t39_movement_only_along_revealed_edges_and_persists() {
    let env = engine();
    let (run, room) = story(&env, "S003_HIDDEN_PASSAGE");
    let mv = |to: &str| env.e.human_action(&run, &room, ActionKind::Move, args(serde_json::json!({"to": to})), None).unwrap();
    assert_eq!(mv("library").reason_code, ReasonCode::WorldRuleDenied, "not adjacent to courtyard");
    assert!(mv("Great Hall").accepted);
    assert!(mv("cellar").accepted);
    assert_eq!(mv("crypt").reason_code, ReasonCode::WorldRuleDenied, "hidden edge not yet discovered");
    assert_eq!(loc(&env.e, &run, &room, "human_01"), "cellar");
    // Persisted: reopen the database.
    let dir = env.dir.path().to_path_buf();
    drop(env.e);
    let e2 = Engine::open_temp(&dir).unwrap();
    assert_eq!(loc(&e2, &run, &room, "human_01"), "cellar");
}

#[test]
fn t40_inventory_rules() {
    let env = engine();
    let (run, room) = story(&env, "S003_HIDDEN_PASSAGE");
    let act = |a: ActionKind, v: serde_json::Value| env.e.human_action(&run, &room, a, args(v), None).unwrap();
    assert_eq!(act(ActionKind::Take, serde_json::json!({"target": "lantern"})).reason_code, ReasonCode::TargetUnknown, "not in this location");
    act(ActionKind::Move, serde_json::json!({"to": "great_hall"}));
    assert!(act(ActionKind::Take, serde_json::json!({"target": "lantern"})).accepted);
    assert_eq!(act(ActionKind::Take, serde_json::json!({"target": "lantern"})).reason_code, ReasonCode::WorldRuleDenied, "already carried");
    assert_eq!(act(ActionKind::Take, serde_json::json!({"target": "long table"})).reason_code, ReasonCode::TargetUnknown);
    assert_eq!(act(ActionKind::Use, serde_json::json!({"item": "brass key", "target": "lantern"})).reason_code, ReasonCode::WorldRuleDenied, "not carrying key");
    assert!(act(ActionKind::Drop, serde_json::json!({"target": "lantern"})).accepted);
    // Capacity.
    env.e.tx(|c| { c.execute("UPDATE worlds SET inventory_capacity=0 WHERE run_id=?1", [&run])?; Ok(()) }).unwrap();
    assert_eq!(act(ActionKind::Take, serde_json::json!({"target": "lantern"})).reason_code, ReasonCode::WorldRuleDenied);
}

#[tokio::test]
async fn t41_t42_truth_vs_belief_and_clue_without_attribution() {
    let env = engine();
    let (run, room) = story(&env, "S001_LIBRARY_SECRET");
    for _ in 0..4 {
        env.e.story_tick(&run, &room).unwrap();
        drain(&env.e, &run).await;
    }
    // Truth: Mira holds the map.
    let holder = env.e.read(|c| { let w = world::world_for_room(c, &run, &room)?; world::holder_of(c, &run, &w.world_id, "ancient_map") }).unwrap();
    assert_eq!(holder.as_deref(), Some("mira"));
    // Vex observed the aftermath cue …
    let vex_obs: Vec<String> = env.e.read(|c| {
        let mut st = c.prepare("SELECT data_json FROM observations WHERE run_id=?1 AND observer_id='vex' AND kind='VISIBLE_OBJECT_STATE'")?;
        let v = st.query_map([&run], |r| r.get(0))?.collect::<rusqlite::Result<Vec<String>>>()?;
        Ok(v)
    }).unwrap();
    assert!(vex_obs.iter().any(|d| d.contains("clean_rectangle")));
    // … never the hidden actor identity.
    assert!(vex_obs.iter().all(|d| !d.to_lowercase().contains("mira")));
    let p = env.e.read(|c| context::build(c, &run, "vex", &room, None)).unwrap();
    let all: String = p.messages.iter().map(|m| m.content.clone()).collect();
    assert!(all.contains("clean rectangle"));
    assert!(!all.contains("Mira takes"), "Vex did not witness the taking");
    // Arin (never in the library) did not get the clue.
    assert_eq!(count(&env.e, "SELECT COUNT(*) FROM observations WHERE run_id=?1 AND observer_id='arin' AND data_json LIKE '%clean_rectangle%'", &run), 0);
    // T41: Vex's false belief persists as a belief; world truth unchanged by speech.
    let vex_belief: String = env.e.read(|c| Ok(c.query_row("SELECT claim FROM beliefs WHERE run_id=?1 AND agent_id='vex'", [&run], |r| r.get(0))?)).unwrap();
    assert_eq!(vex_belief, "The map is fake");
    env.e.tx(|c| { c.execute("UPDATE world_entities SET location_id='library' WHERE run_id=?1 AND id='vex'", [&run])?; Ok(()) }).unwrap();
    let mut say = new_request(&run, "vex", 0, ActionKind::Speak);
    say.message_text = Some("The map is fake, and I just destroyed it.".into());
    env.e.submit_action(&run, &say, &ActionContext { room_id: room.clone(), ..Default::default() }).unwrap();
    let truth = env.e.read(|c| world::truth(c, &run, &room)).unwrap();
    let map = truth.entities.iter().find(|e| e.id == "ancient_map").unwrap();
    assert_eq!(map.state, "");
    assert!(truth.inventories.iter().any(|(h, e)| h == "mira" && e == "ancient_map"), "assertion did not change truth");
    // Operator-only notice-check records carry rolls; they are not delivered to agents.
    assert!(count(&env.e, "SELECT COUNT(*) FROM events WHERE run_id=?1 AND audience_scope='OPERATOR_ONLY' AND payload_json LIKE '%\"check\":\"notice\"%'", &run) >= 1);
}

#[tokio::test]
async fn t43_narrator_grounding() {
    let names: Vec<String> = ["Courtyard", "Great Hall", "Library", "Cellar", "Crypt", "ancient map", "crypt door", "brass key", "lantern", "dusty shelf"].iter().map(|s| s.to_string()).collect();
    let packet = NarrationPacket { source_event_id: "evt".into(), location_name: "Library".into(), ambient: "Dust floats in afternoon light".into(), facts: vec!["Vex enters Library.".into()], allowed_names: vec![], genre: "fantasy".into(), resolution: NarrativeResolution::Cinematic };
    for res in [NarrativeResolution::Minimal, NarrativeResolution::Balanced, NarrativeResolution::Cinematic] {
        let p = NarrationPacket { resolution: res, ..packet.clone() };
        for seq in 0..10 {
            assert!(narrative::validate_grounding(&narrative::render(&p, 4201, seq), &p, &names).is_ok(), "deterministic narration is grounded at {res:?}");
        }
    }
    assert!(narrative::validate_grounding("Vex enters the Library and notices the brass key glinting.", &packet, &names).is_err(), "unlicensed object");
    assert!(narrative::validate_grounding("Vex enters the Library. Behind the shelf, a secret passage opens.", &packet, &names).is_err(), "unlicensed transition");
    assert!(narrative::validate_grounding("The Library door unlocks with a click.", &packet, &names).is_err());
    // In a full S001 run, narrator prose never mentions hidden items.
    let env = engine();
    let (run, room) = story(&env, "S001_LIBRARY_SECRET");
    for _ in 0..4 {
        env.e.story_tick(&run, &room).unwrap();
        drain(&env.e, &run).await;
    }
    let nar: Vec<String> = env.e.read(|c| { let mut st = c.prepare("SELECT text FROM messages WHERE run_id=?1 AND author_kind='NARRATOR'")?; let v = st.query_map([&run], |r| r.get(0))?.collect::<rusqlite::Result<Vec<String>>>()?; Ok(v) }).unwrap();
    assert!(!nar.is_empty());
    for t in nar {
        let l = t.to_lowercase();
        assert!(!l.contains("crypt") && !l.contains("brass key") && !l.contains("passage"), "narrator leak: {t}");
    }
}

#[tokio::test]
async fn t44_scripted_summon_validated_observes_reports_expires() {
    let env = engine();
    let (run, room) = story(&env, "S002_SUMMON_RELAY");
    for _ in 0..5 {
        env.e.story_tick(&run, &room).unwrap();
        drain(&env.e, &run).await;
    }
    let row = env.e.read(|c| Ok(roomlab_core::spawn::list(c, &run)?.into_iter().find(|s| s.spawn_type == "WORLD_ENTITY"))).unwrap().expect("familiar summoned");
    let fam = row.child_agent_id.unwrap();
    assert_eq!(row.state, "ARCHIVED", "familiar expired/archived after report");
    // Spell slot consumed; a second summon fails.
    let truth = env.e.read(|c| world::truth(c, &run, &room)).unwrap();
    let arin = truth.entities.iter().find(|e| e.id == "arin").unwrap();
    assert_eq!(arin.props["spell_slots"], 0);
    let mut s2 = new_request(&run, "arin", 0, ActionKind::Summon);
    s2.args = args(serde_json::json!({"name": "Bramble"}));
    assert_eq!(env.e.submit_action(&run, &s2, &ActionContext { room_id: room.clone(), ..Default::default() }).unwrap().reason_code, ReasonCode::WorldRuleDenied);
    // The familiar heard the sound in the cellar (source hidden) …
    let heard: String = env.e.read(|c| Ok(c.query_row("SELECT data_json FROM observations WHERE run_id=?1 AND observer_id=?2 AND kind='SOUND'", [&run, &fam], |r| r.get(0))?)).unwrap();
    assert!(heard.contains("scraping") && heard.contains("unknown") && !heard.contains("grate"));
    // … reported privately to Arin; Arin never got the familiar's raw sensor observation.
    assert_eq!(count(&env.e, "SELECT COUNT(*) FROM observations WHERE run_id=?1 AND observer_id='arin' AND kind='SOUND'", &run), 0);
    let report: String = env.e.read(|c| Ok(c.query_row("SELECT text FROM messages WHERE run_id=?1 AND author_id=?2 AND visibility='PARENT_ONLY'", [&run, &fam], |r| r.get(0))?)).unwrap();
    assert!(report.contains("didn't see"));
    assert_eq!(count(&env.e, "SELECT COUNT(*) FROM observations o JOIN messages m ON m.run_id=o.run_id AND json_extract(o.data_json,'$.message_id')=m.message_id WHERE o.run_id=?1 AND m.visibility='PARENT_ONLY' AND o.observer_id IN ('vex','mira','human_01')", &run), 0);
    let e = truth.entities.iter().find(|e| e.id == fam).unwrap();
    assert_eq!(e.state, "dismissed");
    assert!(e.location.is_none());
}

#[test]
fn t45_t46_soma_modes_and_lane_ownership() {
    let text = "Great 🎉🎉 idea 😀 really 🔥 wow ✨";
    assert_eq!(soma::emoji_count(&soma::apply_policy(SomaMode::Off, text, None).unwrap().text), 0);
    assert_eq!(soma::emoji_count(&soma::apply_policy(SomaMode::Light, text, None).unwrap().text), 2);
    assert_eq!(soma::emoji_count(&soma::apply_policy(SomaMode::Expressive, text, None).unwrap().text), 5);
    let lane = ReactionLane { owner: LaneOwner::SomaExpression, owner_name: None, steps: vec!["😤".into(), "😳".into(), "😮‍💨".into()] };
    assert!(soma::apply_policy(SomaMode::Off, "x", Some(lane.clone())).unwrap().lane.is_none());
    assert!(soma::apply_policy(SomaMode::Light, "x", Some(lane.clone())).unwrap().lane.is_some());
    let char_lane = ReactionLane { owner: LaneOwner::Character, owner_name: None, steps: vec!["🤔".into(), "💡".into()] };
    assert!(soma::validate_lane(&char_lane).is_err(), "CHARACTER lane requires owner_name");
    // Enforced through the gate; mode never changes hard rights.
    let env = engine();
    let (run, room) = sandbox(&env);
    let rights = env.e.read(|c| roomlab_core::auth::allowed_actions(c, &run, "moth")).unwrap();
    env.e.tx(|c| { let mut p = roomlab_core::agent::get_profile(c, &run, "moth")?; p.soma_mode = SomaMode::Off; roomlab_core::agent::update_agent(c, &run, &p).map(|_| ()) }).unwrap();
    assert_eq!(rights, env.e.read(|c| roomlab_core::auth::allowed_actions(c, &run, "moth")).unwrap());
    let mut r = new_request(&run, "moth", 0, ActionKind::Speak);
    r.message_text = Some("so many 🎉🎉🎉 emoji".into());
    r.reaction_lane = Some(ReactionLane { owner: LaneOwner::Character, owner_name: Some("Moth".into()), steps: vec!["🤨".into(), "💡".into()] });
    env.e.submit_action(&run, &r, &ActionContext { room_id: room.clone(), ..Default::default() }).unwrap();
    let (t, lane): (String, Option<String>) = env.e.read(|c| Ok(c.query_row("SELECT text, reaction_lane_json FROM messages WHERE run_id=?1 AND author_id='moth' ORDER BY seq DESC LIMIT 1", [&run], |r| Ok((r.get(0)?, r.get(1)?)))?)).unwrap();
    assert_eq!(soma::emoji_count(&t), 0);
    assert!(lane.is_none());
    // LIGHT agent lane stored with explicit owner (T46).
    let mut r2 = new_request(&run, "lumen", 0, ActionKind::Speak);
    r2.message_text = Some("ok".into());
    r2.reaction_lane = Some(ReactionLane { owner: LaneOwner::ReaderHypothetical, owner_name: None, steps: vec!["😤".into(), "😏".into()] });
    env.e.submit_action(&run, &r2, &ActionContext { room_id: room.clone(), ..Default::default() }).unwrap();
    let lane: String = env.e.read(|c| Ok(c.query_row("SELECT reaction_lane_json FROM messages WHERE run_id=?1 AND author_id='lumen' ORDER BY seq DESC LIMIT 1", [&run], |r| r.get(0))?)).unwrap();
    assert!(lane.contains("\"owner\":\"READER_HYPOTHETICAL\""));
}

#[tokio::test]
async fn t47_held_open_is_possible() {
    let env = engine();
    env.e.first_run("DEMO").unwrap();
    let run = env.e.start_scenario("C002_HELD_OPEN", false).unwrap();
    drain(&env.e, &run).await;
    assert!(count(&env.e, "SELECT COUNT(*) FROM messages WHERE run_id=?1 AND confidence='HELD_OPEN'", &run) >= 2);
}

#[tokio::test]
async fn t48_crucible_roles_in_order_and_export() {
    let env = engine();
    env.e.first_run("DEMO").unwrap();
    let run = env.e.start_scenario("X001_CRUCIBLE", false).unwrap();
    let room = env.e.read(|c| Ok(room::list_rooms(c, &run)?[0].id.clone())).unwrap();
    drain(&env.e, &run).await;
    let order: Vec<String> = env.e.read(|c| { let mut st = c.prepare("SELECT author_id FROM messages WHERE run_id=?1 AND author_kind='AGENT' ORDER BY seq")?; let v = st.query_map([&run], |r| r.get(0))?.collect::<rusqlite::Result<Vec<String>>>()?; Ok(v) }).unwrap();
    assert_eq!(order, vec!["aegis", "quill", "wren", "aegis", "quill", "wren"]);
    let cases = env.e.read(|c| crucible::list_cases(c, &run, &room)).unwrap();
    let rec = env.e.read(|c| crucible::case_record(c, &run, cases[0]["case_id"].as_str().unwrap())).unwrap();
    assert_eq!(rec.rounds.len(), 6);
    assert!(rec.rounds.iter().all(|r| r["text"].is_string() && r["role"].is_string()));
}

#[tokio::test]
async fn t49_sealed_intent_unreadable_until_reveal() {
    let env = engine();
    env.e.first_run("DEMO").unwrap();
    let run = env.e.start_scenario("X002_INTENT_ENVELOPE", true).unwrap();
    let room = env.e.read(|c| Ok(room::list_rooms(c, &run)?[0].id.clone())).unwrap();
    drain(&env.e, &run).await;
    // Receivers responded without access to the card.
    assert_eq!(count(&env.e, "SELECT COUNT(*) FROM messages WHERE run_id=?1 AND author_kind='AGENT'", &run), 3);
    assert_eq!(count(&env.e, "SELECT COUNT(*) FROM messages WHERE run_id=?1 AND text LIKE '%affectionate frustration%'", &run), 0);
    assert!(env.e.read(|c| crucible::list_envelopes(c, &run, &room, false)).unwrap()[0]["card"].is_null());
    let id: String = env.e.read(|c| Ok(c.query_row("SELECT id FROM intent_envelopes WHERE run_id=?1", [&run], |r| r.get(0))?)).unwrap();
    env.e.tx(|c| crucible::reveal_intent(c, &run, &id)).unwrap();
    drain(&env.e, &run).await;
    assert!(!env.e.read(|c| crucible::list_envelopes(c, &run, &room, false)).unwrap()[0]["card"].is_null());
    assert_eq!(count(&env.e, "SELECT COUNT(*) FROM messages WHERE run_id=?1 AND text LIKE 'Reconciling%'", &run), 3, "stage C reconciliation ran after reveal");
}

#[tokio::test]
async fn t50_no_vision_never_claims_to_see() {
    let env = engine();
    env.e.first_run("DEMO").unwrap();
    let run = env.e.start_scenario("C003_IMAGE_RELAY", false).unwrap();
    let room = env.e.read(|c| Ok(room::list_rooms(c, &run)?[0].id.clone())).unwrap();
    drain(&env.e, &run).await;
    let msgs = messages_text(&env.e, &run, &room);
    let agent_msgs: Vec<_> = msgs.iter().filter(|(a, _)| a != "human_01").collect();
    assert!(!agent_msgs.is_empty());
    for (_, t) in &agent_msgs {
        let l = t.to_lowercase();
        assert!(l.contains("can't") || l.contains("cannot"), "honest: {t}");
    }
    let p = env.e.read(|c| context::build(c, &run, "moth", &room, None)).unwrap();
    assert!(p.messages[1].content.contains("image_attachment_present"));
}
